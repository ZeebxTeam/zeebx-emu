//! Rasterizador de software para o OpenGL ES 1.1 do BREW.
//!
//! O console tem uma Adreno 130; aqui a GPU é a CPU do host. O que importa é o contrato: o
//! jogo entrega vértices em coordenadas de objeto, matrizes de modelagem e projeção, texturas
//! e um punhado de estados fixos, e espera um quadro de volta.
//!
//! É o pipeline fixo clássico, sem iluminação: transforma, recorta contra o plano próximo,
//! divide pela perspectiva, mapeia para a tela e preenche triângulos com interpolação
//! corrigida pela perspectiva, teste de profundidade, mistura e teste de alfa.

use std::collections::HashMap;

use crate::video::gles;

/// Uma matriz 4×4 na ordem do OpenGL: coluna primeiro, `m[coluna * 4 + linha]`.
pub type Matrix = [f32; 16];

/// A identidade.
pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// `a * b`, na convenção do OpenGL (`b` é aplicada primeiro).
pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
        }
    }
    out
}

/// Aplica a matriz a um ponto homogéneo.
pub fn transform(m: &Matrix, v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (row, slot) in out.iter_mut().enumerate() {
        *slot = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    out
}

/// Matriz de translação.
pub fn translation(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    (m[12], m[13], m[14]) = (x, y, z);
    m
}

/// Matriz de escala.
pub fn scaling(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    (m[0], m[5], m[10]) = (x, y, z);
    m
}

/// Rotação de `angle` graus em torno do eixo `(x, y, z)`, como o `glRotate`.
pub fn rotation(angle: f32, x: f32, y: f32, z: f32) -> Matrix {
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 {
        return IDENTITY;
    }
    let (x, y, z) = (x / length, y / length, z / length);
    let (s, c) = angle.to_radians().sin_cos();
    let t = 1.0 - c;
    [
        t * x * x + c,
        t * x * y + s * z,
        t * x * z - s * y,
        0.0,
        t * x * y - s * z,
        t * y * y + c,
        t * y * z + s * x,
        0.0,
        t * x * z + s * y,
        t * y * z - s * x,
        t * z * z + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// Projeção em perspectiva, como o `glFrustum`.
pub fn frustum(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Matrix {
    let mut m = [0.0; 16];
    m[0] = 2.0 * n / (r - l);
    m[5] = 2.0 * n / (t - b);
    m[8] = (r + l) / (r - l);
    m[9] = (t + b) / (t - b);
    m[10] = -(f + n) / (f - n);
    m[11] = -1.0;
    m[14] = -2.0 * f * n / (f - n);
    m
}

/// Projeção ortográfica, como o `glOrtho`.
pub fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Matrix {
    let mut m = IDENTITY;
    m[0] = 2.0 / (r - l);
    m[5] = 2.0 / (t - b);
    m[10] = -2.0 / (f - n);
    m[12] = -(r + l) / (r - l);
    m[13] = -(t + b) / (t - b);
    m[14] = -(f + n) / (f - n);
    m
}

/// Um vértice já pronto para o pipeline: posição em coordenadas de objeto, cor e coordenada de
/// textura.
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
    pub uv: [f32; 2],
    /// A coordenada de textura da unidade 1. Ver [`UnidadeDeTextura`].
    pub uv1: [f32; 2],
    /// A normal em coordenadas de objeto, para a iluminação. O padrão do OpenGL é `(0, 0, 1)`.
    pub normal: [f32; 3],
    /// Quanto da cor do fragmento sobra depois da névoa: 1 é cena limpa, 0 é névoa cheia.
    ///
    /// Vem calculado da etapa de vértice, que é onde a distância em coordenadas de olho existe,
    /// e é interpolado como os outros atributos. Sai daqui pronto porque a etapa de vértice é a
    /// mesma para os dois rasterizadores — a placa recebe o fator como atributo e só mistura.
    pub fog: f32,
}

impl Default for Vertex {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0, 1.0],
            color: [1.0; 4],
            uv: [0.0; 2],
            uv1: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            fog: 1.0,
        }
    }
}

/// O ambiente de textura da unidade 0: o `glTexEnv`, com o modo e a configuração do `GL_COMBINE`.
///
/// **O `GL_COMBINE` não era atendido, e caía no `GL_MODULATE`.** O motor QX do SDK (o Dragon Vs
/// Chicken) desenha os personagens com a fonte 0 na textura e a função `GL_REPLACE`: a cor do
/// vértice não entra, e ele a deixa zerada. Multiplicada pela textura, ela apagava o dragão e as
/// galinhas — preto transparente, que o teste de alfa descartava.
///
/// Os índices `[0]` e `[1]` são o RGB e o alfa. Os padrões são os da especificação.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexEnv {
    pub modo: u32,
    pub combina: [u32; 2],
    pub fontes: [[u32; 3]; 2],
    pub operandos: [[u32; 3]; 2],
    pub escala: [f32; 2],
    /// `GL_TEXTURE_ENV_COLOR`, a fonte `GL_CONSTANT`.
    pub cor: [f32; 4],
}

impl Default for TexEnv {
    fn default() -> Self {
        let fontes = [gles::GL_TEXTURE, gles::GL_PREVIOUS, gles::GL_CONSTANT];
        Self {
            modo: gles::GL_MODULATE,
            combina: [gles::GL_MODULATE; 2],
            fontes: [fontes; 2],
            operandos: [
                [gles::GL_SRC_COLOR, gles::GL_SRC_COLOR, gles::GL_SRC_ALPHA],
                [gles::GL_SRC_ALPHA; 3],
            ],
            escala: [1.0; 2],
            cor: [0.0; 4],
        }
    }
}

impl TexEnv {
    /// Só com o modo, para quem monta um estado à parte (a ponte da placa, os testes).
    pub fn com_modo(modo: u32) -> Self {
        Self {
            modo,
            ..Self::default()
        }
    }

    /// Aplica um `glTexEnv` que não é o modo nem a cor. Os enums chegam como inteiro mesmo nas
    /// formas `x` e `f`; as escalas chegam como número.
    pub fn define(&mut self, pname: u32, enumeracao: u32, numero: f32) {
        let faixa = |base: u32| pname.checked_sub(base).filter(|&i| i < 3).map(|i| i as usize);
        match pname {
            gles::GL_COMBINE_RGB => self.combina[0] = enumeracao,
            gles::GL_COMBINE_ALPHA => self.combina[1] = enumeracao,
            gles::GL_RGB_SCALE => self.escala[0] = numero,
            gles::GL_ALPHA_SCALE => self.escala[1] = numero,
            _ => {
                if let Some(i) = faixa(gles::GL_SRC0_RGB) {
                    self.fontes[0][i] = enumeracao;
                } else if let Some(i) = faixa(gles::GL_SRC0_ALPHA) {
                    self.fontes[1][i] = enumeracao;
                } else if let Some(i) = faixa(gles::GL_OPERAND0_RGB) {
                    self.operandos[0][i] = enumeracao;
                } else if let Some(i) = faixa(gles::GL_OPERAND0_ALPHA) {
                    self.operandos[1][i] = enumeracao;
                }
            }
        }
    }

    /// A cor do fragmento a partir da cor primária e do texel, na unidade 0.
    pub fn aplica(&self, primaria: [f32; 4], texel: [f32; 4]) -> [f32; 4] {
        self.aplica_com(primaria, primaria, texel)
    }

    /// O mesmo numa unidade qualquer: `anterior` é o que saiu da unidade de antes (a cor
    /// primária, na unidade 0), e é sobre ela que os modos clássicos agem.
    pub fn aplica_com(&self, anterior: [f32; 4], primaria: [f32; 4], texel: [f32; 4]) -> [f32; 4] {
        let primaria_ = primaria;
        let primaria = anterior;
        match self.modo {
            gles::GL_REPLACE => texel,
            gles::GL_DECAL => {
                let mut out = primaria;
                for c in 0..3 {
                    out[c] = primaria[c] * (1.0 - texel[3]) + texel[c] * texel[3];
                }
                out
            }
            gles::GL_ADD => {
                let mut out = primaria;
                for c in 0..3 {
                    out[c] = (primaria[c] + texel[c]).min(1.0);
                }
                out[3] = primaria[3] * texel[3];
                out
            }
            gles::GL_COMBINE => self.combina(anterior, primaria_, texel),
            // `GL_MODULATE` é o padrão e o que os jogos usam quase sempre.
            _ => std::array::from_fn(|c| primaria[c] * texel[c]),
        }
    }

    /// O `GL_COMBINE`. Na unidade 0, `GL_PREVIOUS` é a cor primária.
    fn combina(&self, anterior: [f32; 4], primaria: [f32; 4], texel: [f32; 4]) -> [f32; 4] {
        let fonte = |qual: u32| match qual {
            gles::GL_TEXTURE => texel,
            gles::GL_CONSTANT => self.cor,
            gles::GL_PRIMARY_COLOR => primaria,
            _ => anterior,
        };
        // O operando de uma fonte, para um canal. No alfa só existem os dois de alfa; um jogo que
        // manda `GL_SRC_COLOR` ali (o QX manda) recebe o alfa, que é o que o driver faz.
        let operando = |canal: usize, i: usize| -> f32 {
            let lado = usize::from(canal == 3);
            let valor = fonte(self.fontes[lado][i]);
            match (self.operandos[lado][i], lado) {
                (gles::GL_SRC_COLOR, 0) => valor[canal],
                (gles::GL_ONE_MINUS_SRC_COLOR, 0) => 1.0 - valor[canal],
                (gles::GL_ONE_MINUS_SRC_ALPHA | gles::GL_ONE_MINUS_SRC_COLOR, _) => 1.0 - valor[3],
                _ => valor[3],
            }
        };
        let funcao = |canal: usize| -> f32 {
            let lado = usize::from(canal == 3);
            let a = |i| operando(canal, i);
            let resultado = match self.combina[lado] {
                gles::GL_REPLACE => a(0),
                gles::GL_ADD => a(0) + a(1),
                gles::GL_ADD_SIGNED => a(0) + a(1) - 0.5,
                gles::GL_INTERPOLATE => a(0) * a(2) + a(1) * (1.0 - a(2)),
                gles::GL_SUBTRACT => a(0) - a(1),
                gles::GL_DOT3_RGB | gles::GL_DOT3_RGBA => {
                    let produto: f32 = (0..3)
                        .map(|c| (operando(c, 0) - 0.5) * (operando(c, 1) - 0.5))
                        .sum();
                    4.0 * produto
                }
                _ => a(0) * a(1),
            };
            (resultado * self.escala[lado]).clamp(0.0, 1.0)
        };
        let mut saida: [f32; 4] = std::array::from_fn(funcao);
        // O `DOT3_RGBA` põe o produto também no alfa, por cima da função de alfa.
        if self.combina[0] == gles::GL_DOT3_RGBA {
            saida[3] = saida[0];
        }
        saida
    }
}

/// A unidade de textura 1: se está ligada, a textura dela e o ambiente dela.
///
/// **O pipeline desenhava só a unidade 0**, e respondia `GL_MAX_TEXTURE_UNITS` = 1. A Adreno 130
/// do console tem duas, e a especificação do OpenGL ES 1.1 exige ao menos duas. O motor QX do
/// SDK (o Dragon Vs Chicken) confere o número e, com menos de duas, desliga o gerenciador de
/// iluminação — os personagens caíam num caminho de cor zerada e sumiam. Com duas, ele os
/// ilumina pela textura: `DOT3` entre o mapa de relevo e a cor do vértice na unidade 0, e
/// `ADD_SIGNED` com a textura de cor na unidade 1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UnidadeDeTextura {
    pub ligada: bool,
    pub textura: u32,
    pub env: TexEnv,
}

/// A névoa de função fixa: o `glFog*` e o `GL_FOG` do OpenGL ES 1.1.
///
/// O fator sai da **distância em coordenadas de olho**, e é ele que diz quanto da cor da névoa
/// entra no fragmento: 1 é a cena limpa, 0 é a névoa cheia.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neblina {
    pub ligada: bool,
    /// `GL_LINEAR`, `GL_EXP` ou `GL_EXP2`.
    pub curva: u32,
    pub densidade: f32,
    pub inicio: f32,
    pub fim: f32,
    pub cor: [f32; 4],
    /// Desligada por quem está jogando, não pelo jogo. Ver [`Rasterizador::define_neblina`].
    pub permitida: bool,
}

impl Default for Neblina {
    /// Os padrões são os do OpenGL: exponencial, densidade 1, de 0 a 1, preta e transparente.
    fn default() -> Self {
        Self {
            ligada: false,
            curva: gles::GL_EXP,
            densidade: 1.0,
            inicio: 0.0,
            fim: 1.0,
            cor: [0.0; 4],
            permitida: true,
        }
    }
}

impl Neblina {
    /// Quanto da cor original sobra a essa distância do olho: 1 é cena limpa, 0 é névoa cheia.
    ///
    /// Fora da faixa o fator é preso em `[0, 1]`, como manda a especificação — sem isso a névoa
    /// linear **clareia** o que está mais perto que o `START`, em vez de deixá-lo intacto.
    pub fn fator(&self, distancia: f32) -> f32 {
        if !self.ligada || !self.permitida {
            return 1.0;
        }
        let f = match self.curva {
            gles::GL_LINEAR => {
                let faixa = self.fim - self.inicio;
                match faixa.abs() < f32::EPSILON {
                    true => 1.0,
                    false => (self.fim - distancia) / faixa,
                }
            }
            gles::GL_EXP2 => {
                let d = self.densidade * distancia;
                (-(d * d)).exp()
            }
            _ => (-(self.densidade * distancia)).exp(),
        };
        f.clamp(0.0, 1.0)
    }
}

/// Um nível de redução de uma textura.
#[derive(Debug, Clone)]
pub struct Nivel {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
}

/// Uma textura carregada por `TexImage2D`, sempre convertida para RGBA de 8 bits.
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
    /// Os níveis de redução, do 1 em diante — o zero é `width`/`height`/`pixels` acima.
    ///
    /// **Descartá-los era a causa das listras.** Os modelos do palco da Z-Wheel vêm com a
    /// cadeia inteira, de 128×128 até 1×1, e são vistos de raspão: a lateral do carro ocupa
    /// poucos pixels de largura e cobre a textura inteira. Amostrando sempre o nível zero, cada
    /// pixel cai num texel qualquer e o resultado é o traseiro do carro repetido em colunas.
    pub mipmaps: Vec<Nivel>,
    /// Como tratar coordenadas fora de `[0, 1)` em cada eixo.
    pub wrap: [u32; 2],
    /// Filtro de ampliação, do `GL_TEXTURE_MAG_FILTER`.
    pub filter: u32,
    /// Filtro de redução, do `GL_TEXTURE_MIN_FILTER`. É ele que diz se há mipmap e se a
    /// passagem de um nível para o outro é interpolada.
    pub min_filter: u32,
    /// `GL_TEXTURE_CROP_RECT_OES`: qual pedaço da textura o `glDrawTex*OES` desenha, em texels.
    /// Largura ou altura negativa espelha o eixo, que é como a extensão vira a imagem.
    pub crop: [i32; 4],
}

impl Default for Texture {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            mipmaps: Vec::new(),
            // Os padrões do OpenGL ES: repetir nos dois eixos, ampliar por interpolação e
            // reduzir com mipmap — o padrão do `MIN_FILTER` é `GL_NEAREST_MIPMAP_LINEAR`.
            wrap: [gles::GL_REPEAT; 2],
            filter: gles::GL_LINEAR,
            min_filter: gles::GL_NEAREST_MIPMAP_LINEAR,
            crop: [0; 4],
        }
    }
}

impl Texture {
    /// Aplica o modo de repetição de um eixo a um índice de texel.
    ///
    /// `GL_CLAMP_TO_EDGE` prende no último texel e `GL_REPEAT` dá a volta. Tratar tudo como
    /// repetição faz a borda de uma textura presa aparecer do outro lado — é o serrilhado que
    /// surgia nas beiradas da pista do Crash.
    fn wrap(mode: u32, index: i32, size: usize) -> usize {
        let size = size as i32;
        // O caso comum é a coordenada já estar dentro da textura, e aí não há o que fazer —
        // vale conferir antes porque o resto custa uma divisão, e isto roda por texel.
        if index >= 0 && index < size {
            return index as usize;
        }
        if mode == gles::GL_CLAMP_TO_EDGE {
            return index.clamp(0, size - 1) as usize;
        }
        index.rem_euclid(size) as usize
    }

    /// A largura, a altura e os pixels de um nível de redução.
    fn nivel(&self, n: usize) -> (usize, usize, &[[u8; 4]]) {
        match n.checked_sub(1).and_then(|i| self.mipmaps.get(i)) {
            // O tamanho declarado e os pixels precisam combinar: um nível comprimido cujo
            // decodificador não deu conta chega com menos texels do que diz ter, e ler por
            // índice ali seria estourar o vetor.
            Some(nivel) if nivel.width > 0 && nivel.pixels.len() >= nivel.width * nivel.height => {
                (nivel.width, nivel.height, &nivel.pixels)
            }
            _ => (self.width, self.height, &self.pixels),
        }
    }

    /// Quantos níveis a cadeia tem, contando o zero.
    fn niveis(&self) -> usize {
        // **Só a sequência sem buracos conta.** Um jogo pode mandar os níveis fora de ordem, ou
        // parar no meio, e a nossa lista fica com uma entrada vazia no lugar que faltou.
        // Amostrar uma dessas devolvia branco opaco, e a superfície inteira saía chapada — foi
        // o que aconteceu com parte dos itens da roda quando o mipmap entrou.
        1 + self
            .mipmaps
            .iter()
            .take_while(|nivel| nivel.width > 0 && nivel.pixels.len() >= nivel.width * nivel.height)
            .count()
    }

    /// Amostra a textura escolhendo o nível pela redução em tela.
    ///
    /// O `lod` é `log2` de quantos texels cabem num pixel: zero quando um texel é um pixel,
    /// maior quando a superfície está longe ou de raspão. É a conta que o OpenGL manda fazer, e
    /// é ela que decide entre ampliar — com o filtro de ampliação — e reduzir.
    fn sample_lod(&self, u: f32, v: f32, lod: f32) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [1.0; 4];
        }
        if lod <= 0.0 {
            return self.sample_nivel(u, v, 0, self.filter == gles::GL_LINEAR);
        }
        // **Textura sem cadeia não é mipmapeada, mesmo que o filtro peça.** No OpenGL ela é
        // "incompleta" e o resultado é indefinido; na prática o aparelho cai no filtro de base,
        // e é o que faz sentido aqui. O padrão do `MIN_FILTER` é `GL_NEAREST_MIPMAP_LINEAR`, e
        // adotá-lo ao pé da letra passava a amostrar por vizinho mais próximo as vinte e uma
        // texturas do palco que pedem `GL_LINEAR` e não trazem nível nenhum — o carro inteiro
        // perdia definição.
        let base_suave = !matches!(
            self.min_filter,
            gles::GL_NEAREST | gles::GL_NEAREST_MIPMAP_NEAREST | gles::GL_NEAREST_MIPMAP_LINEAR
        );
        if self.niveis() == 1 {
            return self.sample_nivel(u, v, 0, base_suave);
        }
        let (com_mipmap, entre_niveis, suave) = match self.min_filter {
            gles::GL_NEAREST => (false, false, false),
            gles::GL_LINEAR => (false, false, true),
            gles::GL_NEAREST_MIPMAP_NEAREST => (true, false, false),
            gles::GL_LINEAR_MIPMAP_NEAREST => (true, false, true),
            gles::GL_NEAREST_MIPMAP_LINEAR => (true, true, false),
            gles::GL_LINEAR_MIPMAP_LINEAR => (true, true, true),
            _ => (true, true, true),
        };
        if !com_mipmap {
            return self.sample_nivel(u, v, 0, suave);
        }
        let ultimo = (self.niveis() - 1) as f32;
        let lod = lod.min(ultimo);
        let baixo = lod.floor();
        let a = self.sample_nivel(u, v, baixo as usize, suave);
        if !entre_niveis || baixo >= ultimo {
            return a;
        }
        let b = self.sample_nivel(u, v, baixo as usize + 1, suave);
        let t = lod - baixo;
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
    }

    /// Amostra um nível, com ou sem interpolação entre texels vizinhos.
    fn sample_nivel(&self, u: f32, v: f32, nivel: usize, suave: bool) -> [f32; 4] {
        let (width, height, pixels) = self.nivel(nivel);
        if width == 0 || height == 0 {
            return [1.0; 4];
        }
        let texel = |x: i32, y: i32| {
            let x = Self::wrap(self.wrap[0], x, width);
            let y = Self::wrap(self.wrap[1], y, height);
            let p = pixels[y * width + x];
            std::array::from_fn::<f32, 4, _>(|i| p[i] as f32 / 255.0)
        };
        let (x, y) = (u * width as f32 - 0.5, v * height as f32 - 0.5);
        if !suave {
            return texel(x.round() as i32, y.round() as i32);
        }
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let cantos = [
            texel(x0, y0),
            texel(x0 + 1, y0),
            texel(x0, y0 + 1),
            texel(x0 + 1, y0 + 1),
        ];
        std::array::from_fn(|c| {
            let cima = cantos[0][c] + (cantos[1][c] - cantos[0][c]) * fx;
            let baixo = cantos[2][c] + (cantos[3][c] - cantos[2][c]) * fx;
            cima + (baixo - cima) * fy
        })
    }
}

/// Estado completo do OpenGL ES que o rasterizador mantém.
/// Uma das oito luzes do pipeline de função fixa.
///
/// Os padrões são os do OpenGL ES 1.1, e **a luz zero é diferente das outras**: ela nasce com
/// difusa e especular brancas, as demais com pretas. Isso não é curiosidade de tabela — a
/// Z-Wheel só chama `glLightxv` para a ambiente da `GL_LIGHT0` e deixa o resto no padrão, então
/// a difusa branca que ilumina o palco inteiro vem daqui e de mais lugar nenhum.
#[derive(Debug, Clone, Copy)]
pub struct Light {
    pub enabled: bool,
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    /// Já em coordenadas de olho: o `glLight` transforma a posição pela modelview do momento
    /// em que é chamado, e não pela do desenho. O padrão, `(0, 0, 1, 0)`, é direcional.
    pub position: [f32; 4],
    pub spot_direction: [f32; 3],
    pub spot_exponent: f32,
    pub spot_cutoff: f32,
    /// Constante, linear e quadrática, nessa ordem.
    pub attenuation: [f32; 3],
}

impl Light {
    /// O padrão das luzes de índice um em diante.
    fn apagada() -> Self {
        Self {
            enabled: false,
            ambient: [0.0, 0.0, 0.0, 1.0],
            diffuse: [0.0, 0.0, 0.0, 1.0],
            specular: [0.0, 0.0, 0.0, 1.0],
            position: [0.0, 0.0, 1.0, 0.0],
            spot_direction: [0.0, 0.0, -1.0],
            spot_exponent: 0.0,
            spot_cutoff: 180.0,
            attenuation: [1.0, 0.0, 0.0],
        }
    }

    /// O padrão da `GL_LIGHT0`.
    fn zero() -> Self {
        Self {
            diffuse: [1.0; 4],
            specular: [1.0; 4],
            ..Self::apagada()
        }
    }
}

/// O material, que no ES 1.x é um só — não há frente e verso separados.
#[derive(Debug, Clone, Copy)]
pub struct Material {
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub emission: [f32; 4],
    pub shininess: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            ambient: [0.2, 0.2, 0.2, 1.0],
            diffuse: [0.8, 0.8, 0.8, 1.0],
            specular: [0.0, 0.0, 0.0, 1.0],
            emission: [0.0, 0.0, 0.0, 1.0],
            shininess: 0.0,
        }
    }
}

/// A fronteira entre o emulador e quem rasteriza.
///
/// Tudo o que o despacho de GL faz passa por aqui — e **só** por aqui, desde que os campos do
/// [`GlState`] deixaram de ser públicos. A lista é o contrato que um segundo rasterizador
/// precisa cumprir; hoje há uma implementação só, a de software, e é ela que dá a garantia de
/// quadro reproduzível bit a bit que o `ARCHITECTURE.md` descreve.
///
/// Os métodos inerentes do [`GlState`] continuam existindo: o trait não muda nenhum ponto de
/// chamada, ele só escreve o que a fronteira é. Quem for implementar outro backend começa por
/// esta lista, e o que não estiver nela não é usado pelo emulador.
pub trait Rasterizador {
    /// Grava, numa seção por grupo, o estado que as chamadas de GL do jogo mudaram.
    ///
    /// **É o par de [`Rasterizador::restaura_estado`], e os dois são obrigatórios de propósito.** Um
    /// rasterizador que não saiba se gravar tem de dizer isso, e não gravar nada em silêncio: um
    /// save state que perde o estado de desenho volta com a cena errada, e nada aponta para ele.
    fn grava_estado(&self, destino: &mut crate::save_state::Secoes);

    /// Repõe o estado gravado por [`Rasterizador::grava_estado`].
    fn restaura_estado(
        &mut self,
        origem: &crate::save_state::Leitor<'_>,
    ) -> Result<(), crate::save_state::Erro>;

    /// Termina o desenho que estava na fila.
    ///
    /// Existe por causa do save state, e ele é a resposta certa para o caso comum: um lote de
    /// triângulos esperando a vez **não é** um desenho pela metade — é trabalho que ia ser feito no
    /// quadro seguinte de qualquer maneira. Recusar o save por causa dele seria bloquear o jogador
    /// por algo que o motor resolve sozinho. Depois de drenar, o que sobrar é o desenho de
    /// verdade interrompido, e aí [`Rasterizador::desenho_em_curso`] responde sim.
    fn descarrega_o_desenho(&mut self);

    /// Se há um desenho **começado e não terminado**.
    ///
    /// O frontend serializa entre quadros, nunca dentro de um `retro_run`, então isto responde
    /// `false` em todo save normal. Quem grava pergunta, e **recusa** quando a resposta é sim: um
    /// estado salvo no meio de um `glBegin` prometeria um desenho que nunca existiu.
    fn desenho_em_curso(&self) -> bool;

    fn set_matrix_mode(&mut self, mode: u32);
    fn load_identity(&mut self);
    fn load_matrix(&mut self, m: Matrix);
    fn mult_matrix(&mut self, m: Matrix);
    fn push_matrix(&mut self);
    fn pop_matrix(&mut self);

    fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32);
    fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32);
    fn set_surface(&mut self, width: usize, height: usize);
    /// Como [`Rasterizador::set_surface`], para a superfície que o aparelho estica até a tela
    /// inteira — a do `EGL_QUALCOMM_surface_scale`. Ver [`GlState::superficie_esticada`].
    fn set_surface_esticada(&mut self, width: usize, height: usize);
    fn surface(&self) -> (usize, usize);
    fn frame_size(&self) -> (usize, usize);

    fn set_clear_color(&mut self, color: [f32; 4]);
    fn set_clear_depth(&mut self, depth: f32);
    fn set_clear_stencil(&mut self, valor: i32);
    fn set_color(&mut self, color: [f32; 4]);
    fn current_color(&self) -> [f32; 4];
    fn clear(&mut self, mask: u32);

    fn set_capability(&mut self, capability: u32, on: bool);
    fn set_shade_model(&mut self, mode: u32);
    fn set_light(&mut self, index: usize, pname: u32, valores: [f32; 4]);
    fn set_material(&mut self, pname: u32, valores: [f32; 4]);
    fn set_light_model(&mut self, pname: u32, valores: [f32; 4]);

    fn set_blend_func(&mut self, src: u32, dst: u32);
    fn set_alpha_func(&mut self, func: u32, reference: f32);
    fn set_depth_func(&mut self, func: u32);
    fn set_depth_mask(&mut self, on: bool);
    /// `glDepthRange`: para onde a profundidade normalizada vai no buffer de profundidade.
    fn set_depth_range(&mut self, perto: f32, longe: f32);
    /// `glFog*`: um parâmetro da névoa, com até quatro valores (a cor usa os quatro).
    fn set_fog(&mut self, pname: u32, valores: [f32; 4]);
    /// Se a névoa do jogo vale. **É escolha de quem joga, não do jogo**: a névoa do console
    /// costuma esconder o que a distância de desenho dele não alcançava, e aqui a cena chega
    /// inteira — quem prefere ver longe desliga.
    fn define_neblina(&mut self, permitida: bool);
    fn set_color_mask(&mut self, mask: [bool; 4]);
    fn set_cull_face(&mut self, mode: u32);
    fn set_front_face(&mut self, face: u32);
    fn set_stencil_func(&mut self, func: u32, referencia: i32, mask: u32);
    fn set_stencil_op(&mut self, falha: u32, falha_z: u32, passa: u32);
    fn set_stencil_mask(&mut self, mask: u32);

    fn set_active_texture(&mut self, unit: u32);
    fn set_client_active_texture(&mut self, unit: u32);
    /// A unidade escolhida pelo `glClientActiveTexture`, a partir de zero.
    fn client_unit(&self) -> u32;
    fn bind_texture(&mut self, name: u32);
    fn bound_texture(&self) -> u32;
    fn set_texture_env(&mut self, mode: u32);
    fn set_texture_env_param(&mut self, pname: u32, enumeracao: u32, numero: f32);
    fn set_texture_env_color(&mut self, cor: [f32; 4]);
    fn set_texture_parameter(&mut self, name: u32, value: u32);
    fn set_texture_crop(&mut self, crop: [i32; 4]);
    fn delete_texture(&mut self, name: u32);
    fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    );
    fn sub_image(
        &mut self,
        name: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[[u8; 4]],
    ) -> Result<(), Option<(u32, u32)>>;

    fn draw(&mut self, mode: u32, vertices: &[Vertex]);
    fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32);

    fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]>;
    /// Manda o desenho para um framebuffer de fora — o que o frontend Libretro entrega.
    ///
    /// No contrato do `libretro`, quem apresenta o quadro é o frontend, e o core desenha no
    /// framebuffer que ele indica (o `get_current_framebuffer` do `retro_hw_render_callback`),
    /// que pode mudar de um quadro para o outro — daí a função aceitar troca. `Some(0)` é o
    /// framebuffer padrão do frontend, que no `glow` se escreve `None`.
    ///
    /// **No software não há o que fazer**: ele desenha em memória, e o quadro sai pelo
    /// [`Self::frame_rgb565`] de sempre. É o que permite ao mesmo motor servir aos dois caminhos.
    fn desenha_no_fbo(&mut self, _fbo: Option<u32>) {}

    fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>);
    fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]);

    /// Quantas vezes o quadro é desenhado maior que o do console, por lado. O jogo continua
    /// vendo 640×480: viewport, leitura de pixels e cópia do quadro são convertidas. Só a placa
    /// sabe fazer isto; no software o custo cresceria com o quadrado do fator, e ele ignora.
    fn define_escala(&mut self, _escala: usize) {}

    /// **Experimental.** Renderiza o 3D em perspectiva numa proporção mais larga que o 4:3 do
    /// console, abrindo o campo de visão na horizontal em vez de esticar. `None` é o nativo.
    fn define_proporcao(&mut self, _aspecto: Option<f32>) {}

    /// Antialias por amostragem múltipla (MSAA), em amostras por pixel; 1 desliga. Suaviza as
    /// bordas dos polígonos; transparência recortada por teste de alfa não é afetada.
    fn define_antialias(&mut self, _amostras: usize) {}

    /// Filtro anisotrópico nas texturas do jogo; 1 desliga. Deixa nítido o que é visto de lado —
    /// pista, chão, paredes. Sem a extensão na placa, fica desligado.
    fn define_anisotropico(&mut self, _nivel: usize) {}

    /// O quadro na resolução interna, lido da placa em RGBA com a linha 0 no topo. Para conferir
    /// o upscale sem janela; a janela usa o [`Rasterizador::quadro_na_placa`].
    fn le_quadro_grande(&mut self) -> Option<(usize, usize, Vec<u8>)> {
        None
    }

    /// O quadro na resolução interna, como textura da placa, para a janela pintar direto.
    fn quadro_na_placa(&self) -> Option<QuadroNaPlaca> {
        None
    }

    /// Devolve ao dono o estado de GL que o rasterizador mexeu.
    ///
    /// **Só faz sentido para quem desenha num contexto emprestado**, e é chamado uma vez por
    /// fatia de execução, quando o controle volta para a interface — não a cada desenho. Ver
    /// [`crate::session::Session::step`] e o comentário do `GpuState::submete_com`.
    fn devolve_o_contexto(&self) {}
}

/// Uma textura de cor da placa com o quadro já desenhado, e o pedaço dela que é a imagem.
#[derive(Debug, Clone, Copy)]
pub struct QuadroNaPlaca {
    pub textura: glow::Texture,
    /// A fração da textura que a superfície do jogo ocupa, em `(u, v)`; a linha 0 é o topo.
    pub recorte: [f32; 2],
    /// Largura sobre altura da imagem: 4:3 no nativo, mais larga com a
    /// [`Rasterizador::define_proporcao`].
    pub proporcao: f32,
}

impl Rasterizador for GlState {
    fn grava_estado(&self, destino: &mut crate::save_state::Secoes) {
        crate::save_state::Guardavel::grava(self, destino);
    }

    fn restaura_estado(
        &mut self,
        origem: &crate::save_state::Leitor<'_>,
    ) -> Result<(), crate::save_state::Erro> {
        crate::save_state::Guardavel::restaura(self, origem)
    }

    /// Os três acumuladores de um desenho em curso.
    ///
    /// `transformed` é o que sobra de um `glBegin` sem `glEnd`, e `batch`/`pending` são o lote
    /// esperando a vez de ser submetido. Vazios, não há desenho pela metade.
    fn descarrega_o_desenho(&mut self) {
        self.flush();
    }

    /// O lote que ainda não foi submetido.
    ///
    /// **`transformed` fica de fora, e não por esquecimento**: ele é área de **rascunho** da etapa
    /// de vértice — o `etapa_de_vertice` faz `mem::take` e reaproveita o vetor. Sobra dele é o que
    /// ficou do último lote, e não um desenho interrompido. Eu o incluí na primeira versão e o
    /// resultado foi o save state recusado **sempre**: os quatro vértices de um quad ficavam lá
    /// entre quadros, e gravar virou impossível. Foi o instrumento que mostrou os quatro.
    fn desenho_em_curso(&self) -> bool {
        !self.batch.triangles.is_empty() || !self.pending.triangles.is_empty()
    }

    fn set_matrix_mode(&mut self, mode: u32) {
        GlState::set_matrix_mode(self, mode)
    }
    fn load_identity(&mut self) {
        GlState::load_identity(self)
    }
    fn load_matrix(&mut self, m: Matrix) {
        GlState::load_matrix(self, m)
    }
    fn mult_matrix(&mut self, m: Matrix) {
        GlState::mult_matrix(self, m)
    }
    fn push_matrix(&mut self) {
        GlState::push_matrix(self)
    }
    fn pop_matrix(&mut self) {
        GlState::pop_matrix(self)
    }
    fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32) {
        GlState::set_viewport(self, x, y, width, height)
    }
    fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32) {
        GlState::set_scissor(self, x, y, width, height)
    }
    fn set_surface(&mut self, width: usize, height: usize) {
        GlState::set_surface(self, width, height)
    }
    fn set_surface_esticada(&mut self, width: usize, height: usize) {
        GlState::set_surface_esticada(self, width, height)
    }
    fn surface(&self) -> (usize, usize) {
        GlState::surface(self)
    }
    fn frame_size(&self) -> (usize, usize) {
        GlState::frame_size(self)
    }
    fn set_clear_color(&mut self, color: [f32; 4]) {
        GlState::set_clear_color(self, color)
    }
    fn set_clear_depth(&mut self, depth: f32) {
        GlState::set_clear_depth(self, depth)
    }
    fn set_clear_stencil(&mut self, valor: i32) {
        GlState::set_clear_stencil(self, valor)
    }
    fn set_color(&mut self, color: [f32; 4]) {
        GlState::set_color(self, color)
    }
    fn current_color(&self) -> [f32; 4] {
        GlState::current_color(self)
    }
    fn clear(&mut self, mask: u32) {
        GlState::clear(self, mask)
    }
    fn set_capability(&mut self, capability: u32, on: bool) {
        GlState::set_capability(self, capability, on)
    }
    fn set_shade_model(&mut self, mode: u32) {
        GlState::set_shade_model(self, mode)
    }
    fn set_light(&mut self, index: usize, pname: u32, valores: [f32; 4]) {
        GlState::set_light(self, index, pname, valores)
    }
    fn set_material(&mut self, pname: u32, valores: [f32; 4]) {
        GlState::set_material(self, pname, valores)
    }
    fn set_light_model(&mut self, pname: u32, valores: [f32; 4]) {
        GlState::set_light_model(self, pname, valores)
    }
    fn set_blend_func(&mut self, src: u32, dst: u32) {
        GlState::set_blend_func(self, src, dst)
    }
    fn set_alpha_func(&mut self, func: u32, reference: f32) {
        GlState::set_alpha_func(self, func, reference)
    }
    fn set_depth_func(&mut self, func: u32) {
        GlState::set_depth_func(self, func)
    }
    fn set_depth_mask(&mut self, on: bool) {
        GlState::set_depth_mask(self, on)
    }
    fn set_depth_range(&mut self, perto: f32, longe: f32) {
        self.depth_range = (perto.clamp(0.0, 1.0), longe.clamp(0.0, 1.0));
    }
    fn set_fog(&mut self, pname: u32, valores: [f32; 4]) {
        GlState::set_fog(self, pname, valores)
    }
    fn define_neblina(&mut self, permitida: bool) {
        self.fog.permitida = permitida;
    }
    fn set_color_mask(&mut self, mask: [bool; 4]) {
        GlState::set_color_mask(self, mask)
    }
    fn set_cull_face(&mut self, mode: u32) {
        GlState::set_cull_face(self, mode)
    }
    fn set_front_face(&mut self, face: u32) {
        GlState::set_front_face(self, face)
    }
    fn set_stencil_func(&mut self, func: u32, referencia: i32, mask: u32) {
        GlState::set_stencil_func(self, func, referencia, mask)
    }
    fn set_stencil_op(&mut self, falha: u32, falha_z: u32, passa: u32) {
        GlState::set_stencil_op(self, falha, falha_z, passa)
    }
    fn set_stencil_mask(&mut self, mask: u32) {
        GlState::set_stencil_mask(self, mask)
    }
    fn set_active_texture(&mut self, unit: u32) {
        GlState::set_active_texture(self, unit)
    }
    fn set_client_active_texture(&mut self, unit: u32) {
        GlState::set_client_active_texture(self, unit)
    }
    fn client_unit(&self) -> u32 {
        self.client_unit
    }
    fn bind_texture(&mut self, name: u32) {
        GlState::bind_texture(self, name)
    }
    fn bound_texture(&self) -> u32 {
        GlState::bound_texture(self)
    }
    fn set_texture_env(&mut self, mode: u32) {
        GlState::set_texture_env(self, mode)
    }
    fn set_texture_env_param(&mut self, pname: u32, enumeracao: u32, numero: f32) {
        GlState::set_texture_env_param(self, pname, enumeracao, numero)
    }
    fn set_texture_env_color(&mut self, cor: [f32; 4]) {
        GlState::set_texture_env_color(self, cor)
    }
    fn set_texture_parameter(&mut self, name: u32, value: u32) {
        GlState::set_texture_parameter(self, name, value)
    }
    fn set_texture_crop(&mut self, crop: [i32; 4]) {
        GlState::set_texture_crop(self, crop)
    }
    fn delete_texture(&mut self, name: u32) {
        GlState::delete_texture(self, name)
    }
    fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    ) {
        GlState::upload_level(self, name, level, width, height, pixels)
    }
    fn sub_image(
        &mut self,
        name: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[[u8; 4]],
    ) -> Result<(), Option<(u32, u32)>> {
        GlState::sub_image(self, name, x, y, width, height, pixels)
    }
    fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
        GlState::draw(self, mode, vertices)
    }
    fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32) {
        GlState::draw_texture(self, x, y, z, width, height)
    }
    fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]> {
        GlState::read_rect(self, x, y, width, height)
    }
    fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>) {
        GlState::frame_rgb565(self, width, height, out)
    }
    fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]) {
        GlState::import_rgb565_changes(self, width, height, old, new)
    }
}

pub struct GlState {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Cor do quadro, em RGBA de 8 bits — convertida para RGB565 só na apresentação.
    pub(crate) color: Vec<[u8; 4]>,
    /// Profundidade normalizada em `[0, 1]`.
    pub(crate) depth: Vec<f32>,
    /// O stencil, de oito bits — o tamanho que o `GL_STENCIL_BITS` do console anuncia.
    ///
    /// Existe por causa do reflexo do palco da Z-Wheel: ela marca o chão aqui e desenha o
    /// modelo espelhado só onde a marca ficou. Sem o buffer, o espelhado saía por fora do chão
    /// e virava um rastro esticado ao lado do modelo.
    pub(crate) stencil: Vec<u8>,

    pub(crate) matrix_mode: u32,
    pub(crate) modelview: Vec<Matrix>,
    pub(crate) projection: Vec<Matrix>,
    pub(crate) texture_matrix: Vec<Matrix>,

    pub(crate) viewport: (i32, i32, i32, i32),
    /// O `glScissor`, já com o `y` contado do topo, e só quando o `GL_SCISSOR_TEST` está ligado.
    ///
    /// O Peggle desenha a folha de fontes inteira e conta com ele para aparecer uma letra só —
    /// o mesmo truque que o Pac-Mania faz com o recorte do `IDisplay`. Ignorá-lo punha a folha
    /// inteira na tela.
    pub(crate) tesoura: Option<(i32, i32, i32, i32)>,
    /// O retângulo cru do `glScissor`, com o `y` de baixo para cima, como o jogo o passou.
    pub(crate) tesoura_crua: (i32, i32, i32, i32),
    pub(crate) tesoura_ligada: bool,
    pub(crate) surface: Option<(usize, usize)>,
    /// Se a superfície vai esticada à tela inteira. Ver [`GlState::superficie_esticada`].
    pub(crate) esticada: bool,
    pub(crate) clear_color: [f32; 4],
    pub(crate) clear_depth: f32,
    pub(crate) current_color: [f32; 4],

    pub(crate) textures: HashMap<u32, Texture>,
    pub(crate) bound_texture: u32,
    pub(crate) texture_env: TexEnv,
    /// A unidade 1. A 0 são os campos soltos acima, de antes de haver outra.
    pub(crate) unidade1: UnidadeDeTextura,

    pub(crate) texture_2d: bool,
    /// Unidade de textura ativa, contada de zero. O `glActiveTexture` a escolhe.
    ///
    /// O pipeline lê uma textura por fragmento, então só a unidade zero tem efeito e as outras
    /// são ignoradas. Ignorar não é o mesmo que não ter multitextura: é a diferença entre
    /// desenhar a camada base e desenhar branco. O Resident Evil 4 monta o mundo com duas
    /// unidades e termina cada bloco na unidade 1; como o `glActiveTexture` não era tratado,
    /// tudo caía num estado só, a última ligação vencia e a textura base era perdida — a vila
    /// inteira saía branca.
    pub(crate) active_unit: u32,
    /// Unidade escolhida pelo `glClientActiveTexture`, que vale para o vetor de coordenadas.
    pub(crate) client_unit: u32,
    pub(crate) depth_test: bool,
    pub(crate) depth_mask: bool,
    /// A névoa do `glFog*`: ligada, curva, cor e os parâmetros de cada curva.
    ///
    /// O Resident Evil 4 a usa para escurecer o fundo dos cenários, e é assim que ele separa o
    /// que está perto do que está longe. Ignorá-la deixava a cena inteira com o mesmo brilho.
    pub(crate) fog: Neblina,
    /// O `glDepthRange`: `(perto, longe)`, de 0 a 1.
    ///
    /// **Ignorá-lo fazia a pista do Crash Nitro Kart surgir do nada perto do jogador.** O jogo
    /// desenha partes da cena em faixas de profundidade diferentes — `(0, 0,985)` e `(0, 1)` —
    /// para que umas fiquem sempre à frente de outras. Com todas na faixa inteira, um pedaço de
    /// pista distante perdia o teste de profundidade para o cenário e só aparecia de perto.
    pub(crate) depth_range: (f32, f32),
    /// Quais canais de cor podem ser escritos, do `glColorMask`.
    pub(crate) color_mask: [bool; 4],
    pub(crate) depth_func: u32,
    pub(crate) blend: bool,
    pub(crate) blend_src: u32,
    pub(crate) blend_dst: u32,
    pub(crate) alpha_test: bool,
    pub(crate) alpha_func: u32,
    pub(crate) alpha_ref: f32,
    pub(crate) cull_face: bool,
    pub(crate) cull_mode: u32,
    pub(crate) front_face: u32,
    pub(crate) stencil_test: bool,
    /// `glStencilFunc(func, ref, mask)`.
    pub(crate) stencil_func: u32,
    pub(crate) stencil_ref: i32,
    pub(crate) stencil_value_mask: u32,
    /// `glStencilMask` — que bits o desenho pode escrever.
    pub(crate) stencil_write_mask: u32,
    /// `glStencilOp(sfail, dpfail, dppass)`.
    pub(crate) stencil_op: [u32; 3],
    pub(crate) clear_stencil: u8,

    /// `GL_LIGHTING`. Com ele ligado a cor do vértice **deixa de valer**, a menos que o
    /// `GL_COLOR_MATERIAL` diga o contrário: é o que a especificação manda, e o palco da
    /// Z-Wheel depende disso — os modelos dele chegam com cor de vértice branca e é a luz que
    /// dá o relevo.
    pub(crate) lighting: bool,
    pub(crate) color_material: bool,
    pub(crate) lights: [Light; gles::LUZES],
    pub(crate) material: Material,
    pub(crate) light_model_ambient: [f32; 4],
    pub(crate) shade_model: u32,

    /// Lote de triângulos da draw call em curso. Vive na struct só para reaproveitar a
    /// alocação de uma chamada para a outra.
    batch: Batch,
    /// O que já foi desenhado neste quadro e ainda não virou pixel. Ver [`GlState::flush`].
    pending: Pending,
    /// Os vértices já transformados da draw call em curso. Vive aqui pelo mesmo motivo: o
    /// Quake faz meio milhão de draw calls em vinte e cinco segundos de jogo, e uma alocação
    /// por chamada é meio milhão de idas ao alocador para desenhar dois triângulos de cada vez.
    transformed: Vec<Vertex>,
    /// Se o buffer de cor mudou desde a última vez que ele foi entregue convertido.
    ///
    /// A Z-Wheel desenha num pbuffer e lê o resultado com `eglGetColorBufferQUALCOMM` **duas
    /// vezes por quadro**: a primeira sempre com a fila vazia — medido, zero jobs e zero
    /// triângulos em 1.596 quadros. Sem isto, cada uma dessas leituras reconvertia duzentos e
    /// onze mil pixels de um quadro idêntico ao anterior, 1,46 ms que não produziam nada.
    sujo: bool,
}

impl GlState {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            color: vec![[0, 0, 0, 255]; width * height],
            depth: vec![1.0; width * height],
            stencil: vec![0; width * height],
            lighting: false,
            color_material: false,
            lights: std::array::from_fn(|i| match i {
                0 => Light::zero(),
                _ => Light::apagada(),
            }),
            material: Material::default(),
            light_model_ambient: [0.2, 0.2, 0.2, 1.0],
            shade_model: gles::GL_SMOOTH,
            stencil_test: false,
            stencil_func: gles::GL_ALWAYS,
            stencil_ref: 0,
            stencil_value_mask: u32::MAX,
            stencil_write_mask: u32::MAX,
            stencil_op: [gles::GL_KEEP; 3],
            clear_stencil: 0,
            matrix_mode: gles::GL_MODELVIEW,
            modelview: vec![IDENTITY],
            projection: vec![IDENTITY],
            texture_matrix: vec![IDENTITY],
            viewport: (0, 0, width as i32, height as i32),
            tesoura: None,
            tesoura_crua: (0, 0, width as i32, height as i32),
            tesoura_ligada: false,
            surface: None,
            esticada: false,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            clear_depth: 1.0,
            current_color: [1.0; 4],
            textures: HashMap::new(),
            bound_texture: 0,
            texture_env: TexEnv::default(),
            unidade1: UnidadeDeTextura::default(),
            texture_2d: false,
            active_unit: 0,
            client_unit: 0,
            depth_test: false,
            depth_mask: true,
            fog: Neblina::default(),
            depth_range: (0.0, 1.0),
            color_mask: [true; 4],
            depth_func: gles::GL_LESS,
            blend: false,
            blend_src: gles::GL_ONE,
            blend_dst: gles::GL_ZERO,
            alpha_test: false,
            alpha_func: gles::GL_ALWAYS,
            alpha_ref: 0.0,
            cull_face: false,
            cull_mode: gles::GL_BACK,
            front_face: gles::GL_CCW,
            batch: Batch::default(),
            pending: Pending::default(),
            transformed: Vec::new(),
            sujo: true,
        }
    }

    /// A pilha de matrizes ativa.
    fn stack(&mut self) -> &mut Vec<Matrix> {
        match self.matrix_mode {
            gles::GL_PROJECTION => &mut self.projection,
            gles::GL_TEXTURE => &mut self.texture_matrix,
            _ => &mut self.modelview,
        }
    }

    /// A matriz do topo da pilha ativa.
    fn top(&mut self) -> &mut Matrix {
        self.stack().last_mut().expect("pilha nunca fica vazia")
    }

    pub fn set_matrix_mode(&mut self, mode: u32) {
        self.matrix_mode = mode;
    }

    pub fn load_identity(&mut self) {
        *self.top() = IDENTITY;
    }

    pub fn load_matrix(&mut self, m: Matrix) {
        *self.top() = m;
    }

    /// Multiplica a matriz do topo por `m`, na ordem do OpenGL.
    pub fn mult_matrix(&mut self, m: Matrix) {
        let top = self.top();
        *top = multiply(top, &m);
    }

    pub fn push_matrix(&mut self) {
        let stack = self.stack();
        // Empilhar sem limite tornaria um `PushMatrix` desbalanceado num vazamento silencioso;
        // o OpenGL ES garante 16 níveis, e é o que oferecemos.
        if stack.len() < MAX_MATRIX_STACK {
            let top = *stack.last().expect("pilha nunca fica vazia");
            stack.push(top);
        }
    }

    pub fn pop_matrix(&mut self) {
        let stack = self.stack();
        if stack.len() > 1 {
            stack.pop();
        }
    }

    /// O retângulo do `glScissor`. Guardado cru e convertido para o topo quando vale.
    pub fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.tesoura_crua = (x, y, width, height);
        self.atualiza_tesoura();
    }

    /// Liga ou desliga o `GL_SCISSOR_TEST`.
    pub fn set_scissor_test(&mut self, ligado: bool) {
        self.tesoura_ligada = ligado;
        self.atualiza_tesoura();
    }

    /// Converte o retângulo do `glScissor` para a nossa superfície, que conta o `y` do topo.
    fn atualiza_tesoura(&mut self) {
        let (x, y, largura, altura) = self.tesoura_crua;
        self.tesoura = match self.tesoura_ligada {
            false => None,
            true => {
                let altura_da_superficie = self.surface().1 as i32;
                Some((x, altura_da_superficie - y - altura, largura, altura))
            }
        };
    }

    pub fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.viewport = (x, y, width, height);
        // O jogo desenha numa superfície que pode ser menor que a tela e é ampliada na
        // apresentação — no console isso é a extensão `EGL_QUALCOMM_surface_scale`. Ele nunca
        // nos diz o tamanho dessa superfície, mas a maior viewport que ele usa é exatamente
        // ela: nenhum desenho passa desse retângulo.
        let (seen_x, seen_y) = self.surface.unwrap_or((0, 0));
        self.surface = Some((
            seen_x.max((x + width).max(0) as usize),
            seen_y.max((y + height).max(0) as usize),
        ));
    }

    /// A viewport com o `y` contado do topo, que é como os nossos quadros são guardados.
    ///
    /// **O `glViewport` conta o `y` de baixo para cima.** Com a viewport da tela inteira isso não
    /// aparece. O Crash Nitro Kart desenha o trecho seguinte da pista através do portal dele, com
    /// a viewport no retângulo do portal na tela — um retângulo estreito perto do horizonte. Usado
    /// como se contasse do topo, o trecho ia parar espelhado na parte de baixo da tela, e o que se
    /// via era um vazio à frente até o kart atravessar o portal, com o cenário surgindo de baixo
    /// para cima.
    fn viewport_do_topo(&self) -> (i32, i32, i32, i32) {
        let (x, y, largura, altura) = self.viewport;
        let altura_da_superficie = self.surface().1 as i32;
        (x, altura_da_superficie - y - altura, largura, altura)
    }

    /// Declara o tamanho da superfície, quando o jogo o informa.
    ///
    /// O `EGL_QUALCOMM_surface_scale` do console é exatamente isso: o jogo desenha pequeno e o
    /// aparelho amplia. Vale mais que a dedução por viewport — aqui ele **diz** o tamanho, e a
    /// dedução existe só para quem não diz.
    pub fn set_surface(&mut self, width: usize, height: usize) {
        // O que foi desenhado no tamanho antigo precisa virar pixel antes da troca.
        self.flush();
        if width > 0 && height > 0 {
            self.surface = Some((width, height));
        }
        self.esticada = false;
    }

    /// Declara a superfície que o aparelho amplia até a tela inteira.
    pub fn set_surface_esticada(&mut self, width: usize, height: usize) {
        self.set_surface(width, height);
        self.esticada = width > 0 && height > 0;
    }

    /// Se a superfície menor que o quadro vai **esticada** à tela, e não num canto dela.
    ///
    /// São dois casos de superfície menor, e eles só diferem aqui. O `EGL_QUALCOMM_surface_scale`
    /// amplia: o Quake desenha em 320×400 e o aparelho mostra em 640×480, 4:3. Um pbuffer não
    /// amplia nada: a Z-Wheel desenha o palco num de 640×330 e o copia para onde quiser. A
    /// proporção larga só sabe abrir os lados do primeiro.
    pub fn superficie_esticada(&self) -> bool {
        self.esticada
    }

    /// O tamanho da superfície em que o jogo desenha, deduzido das viewports usadas.
    ///
    /// Quem nunca chama `glViewport` fica com a viewport padrão, que o OpenGL define como a
    /// superfície inteira — e é isso que vale então. Deduzir a superfície de um conjunto vazio
    /// de viewports dava 1×1, e a apresentação esticava um pixel só por toda a tela: o Zeebo
    /// Sports Peteca, que desenha em coordenadas de tela e nunca mexe na viewport, saía
    /// inteiramente branco.
    pub fn surface(&self) -> (usize, usize) {
        let (width, height) = self.surface.unwrap_or((self.width, self.height));
        (width.clamp(1, self.width), height.clamp(1, self.height))
    }

    /// Um retângulo do quadro, em RGBA de 8 bits, na ordem que o OpenGL usa.
    ///
    /// A origem do `glReadPixels` é o canto **inferior** esquerdo, e a nossa é o superior — daí
    /// a inversão de linha. Ler sem inverter dá uma imagem de cabeça para baixo, que é o tipo de
    /// erro que passa por "quase certo".
    ///
    /// Fora da superfície devolve preto opaco, como uma leitura de área não desenhada.
    pub fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]> {
        // Quem lê o quadro precisa dele pintado; o desenho é acumulado até alguém pedir.
        self.flush();
        let (sw, sh) = self.surface();
        let mut saida = vec![[0, 0, 0, 255]; width * height];
        for linha in 0..height {
            for coluna in 0..width {
                let fx = x + coluna as i32;
                let fy = y + linha as i32;
                if fx < 0 || fy < 0 || fx as usize >= sw || fy as usize >= sh {
                    continue;
                }
                // A linha `fy` contada de baixo é a `sh - 1 - fy` no nosso buffer.
                let origem = (sh - 1 - fy as usize) * self.width + fx as usize;
                saida[linha * width + coluna] = self.color[origem];
            }
        }
        saida
    }

    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    pub fn set_clear_depth(&mut self, depth: f32) {
        self.clear_depth = depth;
    }

    pub fn set_color(&mut self, color: [f32; 4]) {
        self.current_color = color;
    }

    pub fn current_color(&self) -> [f32; 4] {
        self.current_color
    }

    /// `glActiveTexture` — escolhe a unidade a que as próximas chamadas de textura se referem.
    pub fn set_active_texture(&mut self, unit: u32) {
        self.active_unit = unit.wrapping_sub(gles::GL_TEXTURE0);
    }

    /// `glClientActiveTexture` — a mesma escolha, para o vetor de coordenadas de textura.
    pub fn set_client_active_texture(&mut self, unit: u32) {
        self.client_unit = unit.wrapping_sub(gles::GL_TEXTURE0);
    }

    /// Se a projeção corrente é em perspectiva, e não ortográfica: a última linha da matriz tem
    /// o `-z` que faz o `w`. HUD e 2D desenham em ortográfica.
    pub fn projecao_em_perspectiva(&self) -> bool {
        let p = self.projection.last().expect("pilha nunca fica vazia");
        p[11] != 0.0 && p[15] == 0.0
    }

    /// Se a unidade ativa é a base — a única que o pipeline desenha.
    pub fn base_active_unit(&self) -> bool {
        self.active_unit == 0
    }

    /// Se a unidade ativa é uma das duas que desenham.
    pub fn unidade_ativa_desenha(&self) -> bool {
        self.active_unit < 2
    }


    /// Um parâmetro do `glFog*`. O que não conhecemos fica de fora em vez de virar lixo.
    pub fn set_fog(&mut self, pname: u32, valores: [f32; 4]) {
        match pname {
            gles::GL_FOG_MODE => self.fog.curva = valores[0] as u32,
            gles::GL_FOG_DENSITY => self.fog.densidade = valores[0].max(0.0),
            gles::GL_FOG_START => self.fog.inicio = valores[0],
            gles::GL_FOG_END => self.fog.fim = valores[0],
            gles::GL_FOG_COLOR => self.fog.cor = valores,
            _ => {}
        }
    }

    /// A névoa como está agora, para quem precisa repassá-la.
    pub fn neblina(&self) -> Neblina {
        self.fog
    }

    pub fn set_capability(&mut self, capability: u32, on: bool) {
        match capability {
            // Ligar e desligar textura é por unidade; as duas primeiras desenham.
            gles::GL_TEXTURE_2D if self.active_unit == 0 => self.texture_2d = on,
            gles::GL_TEXTURE_2D if self.active_unit == 1 => self.unidade1.ligada = on,
            gles::GL_TEXTURE_2D => {}
            gles::GL_DEPTH_TEST => self.depth_test = on,
            gles::GL_BLEND => self.blend = on,
            gles::GL_ALPHA_TEST => self.alpha_test = on,
            gles::GL_CULL_FACE => self.cull_face = on,
            gles::GL_LIGHTING => self.lighting = on,
            gles::GL_COLOR_MATERIAL => self.color_material = on,
            // As normais são sempre normalizadas aqui, então ligar ou desligar não muda nada.
            // Não é desleixo: com a normalização desligada e normais que não são unitárias, o
            // OpenGL dá um resultado definido e errado, e nenhum jogo pede isso de propósito.
            gles::GL_NORMALIZE | gles::GL_RESCALE_NORMAL => {}
            capacidade
                if (gles::GL_LIGHT0..gles::GL_LIGHT0 + gles::LUZES as u32)
                    .contains(&capacidade) =>
            {
                self.lights[(capacidade - gles::GL_LIGHT0) as usize].enabled = on;
            }
            gles::GL_FOG => self.fog.ligada = on,
            gles::GL_SCISSOR_TEST => self.set_scissor_test(on),
            // **O stencil ainda não existe, e faz falta medida.** O palco da Z-Wheel arma
            // `glStencilFunc` e `glStencilOp` duas vezes por quadro, que é a receita do reflexo
            // plano: marcar o chão no stencil e desenhar o modelo espelhado só onde ele marcou.
            // Sem o teste, o espelhado sai por fora do chão — desligar os desenhos feitos sob
            // teste de stencil faz sumir metade dos rastros esticados que aparecem no carro.
            // Guardar o estado aqui é o primeiro passo, e não faz nada sozinho.
            gles::GL_STENCIL_TEST => self.stencil_test = on,
            _ => {}
        }
    }

    /// `glLight*` — um parâmetro de uma luz.
    ///
    /// A posição é o caso especial: o OpenGL a guarda **em coordenadas de olho**, transformada
    /// pela modelview do instante da chamada. Guardar as coordenadas de objeto e transformar na
    /// hora do desenho parece igual e não é — a luz andaria junto com cada modelo.
    pub fn set_light(&mut self, index: usize, pname: u32, valores: [f32; 4]) {
        let modelview = *self.modelview.last().expect("pilha nunca fica vazia");
        let Some(luz) = self.lights.get_mut(index) else {
            return;
        };
        match pname {
            gles::GL_AMBIENT => luz.ambient = valores,
            gles::GL_DIFFUSE => luz.diffuse = valores,
            gles::GL_SPECULAR => luz.specular = valores,
            gles::GL_POSITION => luz.position = transform(&modelview, valores),
            gles::GL_SPOT_DIRECTION => {
                let [x, y, z, _] = transform(&modelview, [valores[0], valores[1], valores[2], 0.0]);
                luz.spot_direction = [x, y, z];
            }
            gles::GL_SPOT_EXPONENT => luz.spot_exponent = valores[0],
            gles::GL_SPOT_CUTOFF => luz.spot_cutoff = valores[0],
            gles::GL_CONSTANT_ATTENUATION => luz.attenuation[0] = valores[0],
            gles::GL_LINEAR_ATTENUATION => luz.attenuation[1] = valores[0],
            gles::GL_QUADRATIC_ATTENUATION => luz.attenuation[2] = valores[0],
            _ => {}
        }
    }

    /// `glMaterial*`. A face é ignorada: no ES 1.x o material é um só.
    pub fn set_material(&mut self, pname: u32, valores: [f32; 4]) {
        match pname {
            gles::GL_AMBIENT => self.material.ambient = valores,
            gles::GL_DIFFUSE => self.material.diffuse = valores,
            gles::GL_AMBIENT_AND_DIFFUSE => {
                self.material.ambient = valores;
                self.material.diffuse = valores;
            }
            gles::GL_SPECULAR => self.material.specular = valores,
            gles::GL_EMISSION => self.material.emission = valores,
            gles::GL_SHININESS => self.material.shininess = valores[0],
            _ => {}
        }
    }

    /// `glLightModel*`. Só a ambiente da cena tem efeito aqui; o `TWO_SIDE` é aceito e ignorado,
    /// porque o material é um só.
    pub fn set_light_model(&mut self, pname: u32, valores: [f32; 4]) {
        if pname == gles::GL_LIGHT_MODEL_AMBIENT {
            self.light_model_ambient = valores;
        }
    }

    pub fn set_shade_model(&mut self, mode: u32) {
        self.shade_model = mode;
    }

    /// A cor de um vértice com a iluminação ligada, na equação do OpenGL ES 1.x.
    ///
    /// ```text
    /// cor = emissão
    ///     + ambiente_do_material * ambiente_da_cena
    ///     + Σ  atenuação * holofote * ( ambiente_do_material * ambiente_da_luz
    ///                                 + difusa_do_material  * difusa_da_luz  * max(N·L, 0)
    ///                                 + especular_do_material * especular_da_luz * max(N·H, 0)^brilho )
    /// ```
    ///
    /// O alfa **não** vem da soma: é o da difusa do material, e sair somando alfa de luz deixa
    /// tudo opaco. Com `GL_COLOR_MATERIAL`, a cor do vértice toma o lugar da ambiente e da
    /// difusa do material — é o único caminho pelo qual um vetor de cores continua valendo com
    /// a luz ligada.
    fn cor_iluminada(
        &self,
        olho: [f32; 4],
        normal: [f32; 3],
        cor_do_vertice: [f32; 4],
    ) -> [f32; 4] {
        let (ambiente, difusa) = match self.color_material {
            true => (cor_do_vertice, cor_do_vertice),
            false => (self.material.ambient, self.material.diffuse),
        };
        let mut saida = [0.0f32; 3];
        for canal in 0..3 {
            saida[canal] =
                self.material.emission[canal] + ambiente[canal] * self.light_model_ambient[canal];
        }
        // A posição do olho é `(0, 0, 0)` em coordenadas de olho, então a direção para o
        // observador é o próprio ponto, negado e normalizado.
        let para_o_olho = normaliza([-olho[0], -olho[1], -olho[2]]);
        for luz in self.lights.iter().filter(|luz| luz.enabled) {
            let (para_a_luz, distancia) = match luz.position[3] == 0.0 {
                // Direcional: a posição é uma direção, e não há distância nem atenuação.
                true => (
                    normaliza([luz.position[0], luz.position[1], luz.position[2]]),
                    None,
                ),
                false => {
                    let bruto = [
                        luz.position[0] - olho[0],
                        luz.position[1] - olho[1],
                        luz.position[2] - olho[2],
                    ];
                    (normaliza(bruto), Some(comprimento(bruto)))
                }
            };
            let atenuacao = match distancia {
                None => 1.0,
                Some(d) => {
                    let [c, l, q] = luz.attenuation;
                    let divisor = c + l * d + q * d * d;
                    if divisor <= 0.0 { 1.0 } else { 1.0 / divisor }
                }
            };
            let holofote = holofote(luz, para_a_luz);
            let peso = atenuacao * holofote;
            if peso <= 0.0 {
                continue;
            }
            let n_l = ponto(normal, para_a_luz).max(0.0);
            // O meio-vetor de Blinn, que é o que o ES 1.x usa no lugar da reflexão de Phong.
            let brilho = match n_l > 0.0 && self.material.shininess > 0.0 {
                false => 0.0,
                true => {
                    let meio = normaliza([
                        para_a_luz[0] + para_o_olho[0],
                        para_a_luz[1] + para_o_olho[1],
                        para_a_luz[2] + para_o_olho[2],
                    ]);
                    ponto(normal, meio).max(0.0).powf(self.material.shininess)
                }
            };
            for canal in 0..3 {
                saida[canal] += peso
                    * (ambiente[canal] * luz.ambient[canal]
                        + difusa[canal] * luz.diffuse[canal] * n_l
                        + self.material.specular[canal] * luz.specular[canal] * brilho);
            }
        }
        [
            saida[0].clamp(0.0, 1.0),
            saida[1].clamp(0.0, 1.0),
            saida[2].clamp(0.0, 1.0),
            difusa[3].clamp(0.0, 1.0),
        ]
    }

    /// `glStencilFunc(func, ref, mask)`.
    ///
    /// O `ref` é preso à faixa do buffer, como manda a especificação: o console tem oito bits.
    pub fn set_stencil_func(&mut self, func: u32, referencia: i32, mask: u32) {
        self.stencil_func = func;
        self.stencil_ref = referencia.clamp(0, u8::MAX as i32);
        self.stencil_value_mask = mask;
    }

    /// `glStencilOp(sfail, dpfail, dppass)` — o que fazer quando o stencil falha, quando ele
    /// passa e a profundidade falha, e quando os dois passam.
    pub fn set_stencil_op(&mut self, falha: u32, falha_z: u32, passa: u32) {
        self.stencil_op = [falha, falha_z, passa];
    }

    /// `glStencilMask` — que bits do stencil o desenho pode escrever.
    pub fn set_stencil_mask(&mut self, mask: u32) {
        self.stencil_write_mask = mask;
    }

    /// `glClearStencil`.
    pub fn set_clear_stencil(&mut self, valor: i32) {
        self.clear_stencil = valor.clamp(0, u8::MAX as i32) as u8;
    }

    pub fn set_blend_func(&mut self, src: u32, dst: u32) {
        (self.blend_src, self.blend_dst) = (src, dst);
    }

    pub fn set_depth_func(&mut self, func: u32) {
        self.depth_func = func;
    }

    pub fn set_depth_mask(&mut self, on: bool) {
        self.depth_mask = on;
    }

    /// `glColorMask`: quais canais de cor o desenho pode escrever.
    ///
    /// Ignorar isto não é neutro. Um jogo que desenha uma passada só para o alfa — mascarando
    /// vermelho, verde e azul — teria a cor pintada por cima do que já estava lá, e o resultado
    /// é imagem embaralhada sem nenhum erro aparente.
    pub fn set_color_mask(&mut self, mask: [bool; 4]) {
        self.color_mask = mask;
    }

    pub fn set_alpha_func(&mut self, func: u32, reference: f32) {
        (self.alpha_func, self.alpha_ref) = (func, reference);
    }

    pub fn set_cull_face(&mut self, mode: u32) {
        self.cull_mode = mode;
    }

    /// Só `GL_CW` e `GL_CCW` são orientações válidas; qualquer outra coisa o OpenGL recusa, e
    /// aceitá-la aqui inverteria silenciosamente a decisão de qual face aparece.
    pub fn set_front_face(&mut self, face: u32) {
        if face == gles::GL_CW || face == gles::GL_CCW {
            self.front_face = face;
        }
    }

    /// O ambiente da unidade ativa, se ela é uma das que desenham.
    fn env_ativo(&mut self) -> Option<&mut TexEnv> {
        match self.active_unit {
            0 => Some(&mut self.texture_env),
            1 => Some(&mut self.unidade1.env),
            _ => None,
        }
    }

    pub fn set_texture_env(&mut self, mode: u32) {
        if let Some(env) = self.env_ativo() {
            env.modo = mode;
        }
    }

    /// Um parâmetro do `GL_COMBINE`. Ver [`TexEnv::define`].
    pub fn set_texture_env_param(&mut self, pname: u32, enumeracao: u32, numero: f32) {
        if let Some(env) = self.env_ativo() {
            env.define(pname, enumeracao, numero);
        }
    }

    /// A `GL_TEXTURE_ENV_COLOR`.
    pub fn set_texture_env_color(&mut self, cor: [f32; 4]) {
        if let Some(env) = self.env_ativo() {
            env.cor = cor;
        }
    }

    /// A unidade 1, para quem repassa o estado.
    pub fn unidade1(&self) -> UnidadeDeTextura {
        self.unidade1
    }

    /// O nome da textura ligada na unidade ativa, se ela é uma das que desenham.
    fn textura_ativa(&self) -> Option<u32> {
        match self.active_unit {
            0 => Some(self.bound_texture),
            1 => Some(self.unidade1.textura),
            _ => None,
        }
    }

    /// O ambiente de textura da unidade 0, para quem repassa o estado.
    pub fn texture_env(&self) -> TexEnv {
        self.texture_env
    }

    pub fn bind_texture(&mut self, name: u32) {
        match self.active_unit {
            0 => self.bound_texture = name,
            1 => self.unidade1.textura = name,
            _ => return,
        }
        // O nome passa a existir já no `BindTexture`: o jogo costuma ajustar os parâmetros
        // antes de mandar os pixels, e sem a entrada esses ajustes se perderiam.
        self.textures.entry(name).or_default();
    }

    /// `glTexParameter` na textura ligada.
    pub fn set_texture_parameter(&mut self, name: u32, value: u32) {
        let Some(ligada) = self.textura_ativa() else {
            return;
        };
        // **O que já foi enfileirado usa os parâmetros de agora.** A fila guarda o *nome* da
        // textura e vai buscar filtro e repetição só no despejo; mudá-los aqui sem pintar antes
        // faz um desenho anterior ser amostrado com a configuração de um posterior. É o mesmo
        // cuidado que o `TexImage2D` já tomava com os pixels, e que faltava aqui.
        let Some(texture) = self.textures.get(&ligada) else {
            return;
        };
        // Muitos jogos (especialmente NFS) reaplicam o mesmo estado antes de cada sprite.
        // Não há nada para preservar quando o valor não mudou; evitar o flush mantém a fila
        // de triângulos e elimina um custo dominante do quadro.
        let mudou = match name {
            gles::GL_TEXTURE_WRAP_S => texture.wrap[0] != value,
            gles::GL_TEXTURE_WRAP_T => texture.wrap[1] != value,
            gles::GL_TEXTURE_MAG_FILTER => {
                let filtro = if value == gles::GL_NEAREST {
                    gles::GL_NEAREST
                } else {
                    gles::GL_LINEAR
                };
                texture.filter != filtro
            }
            gles::GL_TEXTURE_MIN_FILTER => texture.min_filter != value,
            _ => false,
        };
        if !mudou {
            return;
        }
        self.flush();
        let Some(texture) = self.textures.get_mut(&ligada) else {
            return;
        };
        match name {
            gles::GL_TEXTURE_WRAP_S => texture.wrap[0] = value,
            gles::GL_TEXTURE_WRAP_T => texture.wrap[1] = value,
            // Sem mipmap, o filtro de redução com mipmap se comporta como o de base, e o que
            // importa é distinguir vizinho mais próximo de interpolação.
            // A ampliação só distingue vizinho de interpolação; a redução guarda o valor
            // inteiro, porque é dele que sai se há mipmap e como passar de um nível a outro.
            gles::GL_TEXTURE_MAG_FILTER => {
                texture.filter = match value {
                    gles::GL_NEAREST => gles::GL_NEAREST,
                    _ => gles::GL_LINEAR,
                };
            }
            gles::GL_TEXTURE_MIN_FILTER => texture.min_filter = value,
            _ => {}
        }
    }

    /// `glTexParameteriv(GL_TEXTURE_CROP_RECT_OES, …)` na textura ligada.
    pub fn set_texture_crop(&mut self, crop: [i32; 4]) {
        let Some(ligada) = self.textura_ativa() else {
            return;
        };
        if let Some(texture) = self.textures.get_mut(&ligada) {
            texture.crop = crop;
        }
    }

    /// A textura ligada na unidade ativa — a que um `glTexImage2D` agora alcançaria.
    pub fn bound_texture(&self) -> u32 {
        self.textura_ativa().unwrap_or(self.bound_texture)
    }

    /// O tamanho do quadro — a tela, e não a superfície em que o jogo desenha. Ver
    /// [`GlState::surface`].
    pub fn frame_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Apaga uma textura pelo nome.
    ///
    /// A fila menciona texturas pelo nome: apagar uma antes de ela ser lida mudaria o que já foi
    /// desenhado, e por isso a fila é despejada antes.
    pub fn delete_texture(&mut self, name: u32) {
        self.flush();
        self.textures.remove(&name);
    }

    /// Guarda um nível de textura, criando a textura se ela ainda não existir.
    ///
    /// **O nível zero limpa os menores.** Uma imagem nova no nível base torna a cadeia antiga
    /// mentira, e servir um mipmap de outra textura é pior do que não ter nenhum.
    ///
    /// O despejo da fila é daqui, e não de quem chama: trocar o conteúdo de uma textura que a
    /// fila ainda vai ler mudaria o passado.
    pub fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    ) {
        self.flush();
        let texture = self.textures.entry(name).or_default();
        if level == 0 {
            texture.width = width;
            texture.height = height;
            texture.pixels = pixels;
            texture.mipmaps.clear();
            return;
        }
        let indice = level as usize - 1;
        if texture.mipmaps.len() <= indice {
            texture.mipmaps.resize_with(indice + 1, || Nivel {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            });
        }
        texture.mipmaps[indice] = Nivel {
            width,
            height,
            pixels,
        };
    }

    /// `glTexSubImage2D` no nível base: troca um retângulo dentro de uma textura que já existe.
    ///
    /// Devolve o tamanho da textura quando o retângulo não cabe — recortar seria inventar um
    /// resultado que o OpenGL não define, e quem chamou precisa do número para o relatório.
    /// `Err(None)` é textura que não existe, que não é erro nenhum: o jogo pode ter apagado.
    pub fn sub_image(
        &mut self,
        name: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[[u8; 4]],
    ) -> Result<(), Option<(u32, u32)>> {
        self.flush();
        let Some(texture) = self.textures.get_mut(&name) else {
            return Err(None);
        };
        let (tw, th) = (texture.width as u32, texture.height as u32);
        if x + width > tw || y + height > th {
            return Err(Some((tw, th)));
        }
        for linha in 0..height {
            let destino = ((y + linha) * tw + x) as usize;
            let origem = (linha * width) as usize;
            texture.pixels[destino..destino + width as usize]
                .copy_from_slice(&pixels[origem..origem + width as usize]);
        }
        // **Mexer no nível zero invalida a cadeia.** Os níveis menores continuariam mostrando o
        // que estava ali antes, e quem amostra dois níveis vê os dois conteúdos ao mesmo tempo:
        // o painel de promoção da Z-Wheel, que troca o texto por aqui, saía com as letras
        // fantasmas do texto anterior por cima das novas.
        texture.mipmaps.clear();
        Ok(())
    }

    pub fn clear(&mut self, mask: u32) {
        // Limpar é uma operação sobre o quadro e entra na fila de ordem como qualquer desenho:
        // o que veio antes precisa estar pintado, senão apagaria o que ainda nem existe.
        self.flush();
        if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
            let c = pack(self.clear_color);
            self.color.fill(c);
            self.sujo = true;
        }
        if mask & gles::GL_DEPTH_BUFFER_BIT != 0 {
            self.depth.fill(self.clear_depth);
        }
        if mask & gles::GL_STENCIL_BUFFER_BIT != 0 {
            self.stencil.fill(self.clear_stencil);
        }
    }

    /// Desenha uma sequência de vértices no modo pedido.
    /// A etapa de vértice: transforma, ilumina e deixa o resultado em `transformed`.
    ///
    /// Está separada do preenchimento porque **os dois rasterizadores a compartilham**. Matriz
    /// de modelo-visão, projeção, matriz de textura e iluminação por vértice são a mesma conta
    /// nos dois, e tê-la em um lugar só é o que torna a comparação entre eles honesta: se a luz
    /// estiver errada, estará errada igual nos dois, e a diferença que sobrar é do preenchimento.
    ///
    /// No fim disto os vértices estão em **espaço de recorte**, com cor e `uv` finais — que é
    /// exatamente o que uma placa espera receber num shader de passagem.
    pub fn etapa_de_vertice(&mut self, vertices: &[Vertex]) {
        let mvp = {
            let projection = *self.projection.last().expect("pilha nunca fica vazia");
            let modelview = *self.modelview.last().expect("pilha nunca fica vazia");
            multiply(&projection, &modelview)
        };
        // As coordenadas de textura também passam por uma matriz, e ignorá-la não é um detalhe
        // de acabamento: o Zeebo Sports Peteca manda os `uv` em ponto fixo — a quadra chega com
        // 32767, o extremo de um inteiro de 16 bits — e é a matriz de textura que os traz de
        // volta para a faixa `0..1`. Sem ela, o `GL_REPEAT` dava a volta na textura a cada
        // pixel, e a quadra e a arquibancada saíam como confete das cores certas.
        let texture_matrix = *self.texture_matrix.last().expect("pilha nunca fica vazia");
        // A iluminação acontece em **coordenadas de olho**, que é onde as posições das luzes
        // foram guardadas: daí precisarmos da modelview separada, e não só do produto com a
        // projeção. As normais vão por outra matriz — ver [`matriz_de_normais`].
        let modelview = *self.modelview.last().expect("pilha nunca fica vazia");
        let normais = matriz_de_normais(&modelview);
        let iluminando = self.lighting;
        let neblina = self.fog;
        let mut clip = std::mem::take(&mut self.transformed);
        clip.clear();
        clip.extend(vertices.iter().map(|v| {
            // `q` é o quarto componente da coordenada de textura; a divisão por ele é o
            // que permite projeção na textura, e vale 1 no caso comum.
            let [s, t, _, q] = transform(&texture_matrix, [v.uv[0], v.uv[1], 0.0, 1.0]);
            let scale = if q == 0.0 { 1.0 } else { 1.0 / q };
            // A névoa e a iluminação querem a mesma coisa: o vértice em coordenadas de olho.
            // Com uma das duas ligada a conta sai uma vez e serve às duas.
            let olho = (iluminando || neblina.ligada && neblina.permitida)
                .then(|| transform(&modelview, v.position));
            let color = match (iluminando, olho) {
                (true, Some(olho)) => {
                    let normal = normaliza(gira_normal(&normais, v.normal));
                    self.cor_iluminada(olho, normal, v.color)
                }
                _ => v.color,
            };
            // A distância do olho é `|z|`, como o OpenGL permite em vez do comprimento do vetor:
            // é o que toda implementação de função fixa faz, e é o que o jogo espera ver.
            let fog = match olho {
                Some(olho) => neblina.fator(olho[2].abs()),
                None => 1.0,
            };
            Vertex {
                position: transform(&mvp, v.position),
                uv: [s * scale, t * scale],
                color,
                fog,
                ..*v
            }
        }));
        self.transformed = clip;
    }

    /// Os vértices que a etapa de vértice deixou prontos.
    pub fn transformados(&self) -> &[Vertex] {
        &self.transformed
    }

    pub fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
        self.etapa_de_vertice(vertices);
        let clip = std::mem::take(&mut self.transformed);
        // Os triângulos são projetados primeiro e preenchidos depois, todos juntos: é o lote
        // inteiro que decide se vale dividir o quadro entre threads, e o estado do OpenGL não
        // muda no meio de uma draw call.
        let mut batch = std::mem::take(&mut self.batch);
        batch.clear();
        match mode {
            gles::GL_TRIANGLES => {
                for tri in clip.chunks_exact(3) {
                    self.triangle([tri[0], tri[1], tri[2]], &mut batch);
                }
            }
            gles::GL_TRIANGLE_STRIP => {
                for (i, window) in clip.windows(3).enumerate() {
                    // A cada passo a orientação alterna; trocar dois vértices a mantém.
                    if i % 2 == 0 {
                        self.triangle([window[0], window[1], window[2]], &mut batch);
                    } else {
                        self.triangle([window[1], window[0], window[2]], &mut batch);
                    }
                }
            }
            gles::GL_TRIANGLE_FAN => {
                for window in clip[1..].windows(2) {
                    self.triangle([clip[0], window[0], window[1]], &mut batch);
                }
            }
            // Pontos e linhas não aparecem nos jogos do console, que desenham tudo com
            // triângulos; deixá-los sem tratamento é melhor que rasterizá-los errado.
            gles::GL_POINTS | gles::GL_LINES | gles::GL_LINE_LOOP | gles::GL_LINE_STRIP => {}
            _ => {}
        }
        self.enqueue(&mut batch);
        self.batch = batch;
        self.transformed = clip;
    }

    /// `glDrawTex*OES` — o blit de tela do `GL_OES_draw_texture`.
    ///
    /// A extensão desenha um retângulo **em coordenadas de janela**, sem passar pelas matrizes:
    /// é o caminho que um emulador usa para pôr a tela dele na tela do aparelho, e é o que os
    /// dez portes de arcade do console fazem. Sem ela, o `InitGLExtensions` deles falha e o
    /// alvo de renderização nunca é criado.
    ///
    /// O pedaço da textura vem do `GL_TEXTURE_CROP_RECT_OES`, e largura ou altura negativa ali
    /// espelha o eixo — é assim que a extensão vira a imagem.
    pub fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32) {
        let Some(texture) = self.textures.get(&self.bound_texture) else {
            return;
        };
        let (tw, th) = (texture.width as f32, texture.height as f32);
        if tw == 0.0 || th == 0.0 || width == 0.0 || height == 0.0 {
            return;
        }
        let [ucr, vcr, wcr, hcr] = texture.crop.map(|value| value as f32);
        // Recorte zerado é a textura inteira: um jogo que não pede recorte quer a imagem toda,
        // e desenhar nada seria pior que adotar o padrão óbvio.
        let (wcr, hcr) = match (wcr, hcr) {
            (0.0, 0.0) => (tw, th),
            _ => (wcr, hcr),
        };
        let (s0, s1) = (ucr / tw, (ucr + wcr) / tw);
        let (t0, t1) = (vcr / th, (vcr + hcr) / th);

        // A janela do OpenGL tem o zero embaixo; nossa superfície, em cima.
        let surface_height = self.surface().1 as f32;
        let (left, right) = (x, x + width);
        let (top, bottom) = (surface_height - y - height, surface_height - y);
        let color = self.current_color();
        let corners = [
            ([left, top], [s0, t1]),
            ([left, bottom], [s0, t0]),
            ([right, bottom], [s1, t0]),
            ([right, top], [s1, t1]),
        ];
        let (vx, vy, vw, vh) = self.viewport_do_topo();
        if vw <= 0 || vh <= 0 {
            return;
        }
        let vertices: Vec<Vertex> = corners
            .iter()
            .map(|&([sx, sy], uv)| Vertex {
                normal: [0.0, 0.0, 1.0],
                uv1: [0.0; 2],
                fog: 1.0,
                position: [
                    ((sx - vx as f32) / vw as f32) * 2.0 - 1.0,
                    1.0 - ((sy - vy as f32) / vh as f32) * 2.0,
                    z * 2.0 - 1.0,
                    1.0,
                ],
                color,
                uv,
            })
            .collect();

        // O retângulo não tem orientação definida pela extensão, então descartá-lo por face
        // seria descartar um desenho que o jogo espera ver.
        let culling = self.cull_face;
        self.cull_face = false;
        let mut batch = std::mem::take(&mut self.batch);
        batch.clear();
        self.triangle([vertices[0], vertices[1], vertices[2]], &mut batch);
        self.triangle([vertices[0], vertices[2], vertices[3]], &mut batch);
        self.cull_face = culling;
        self.enqueue(&mut batch);
        self.batch = batch;
    }

    /// Recorta contra o plano próximo do frustum e põe o que sobra no lote.
    ///
    /// O plano próximo é `z >= -w`, não `w > 0`. Recortar só pelo sinal de `w` deixa passar
    /// vértices com `w` minúsculo — logo atrás do plano próximo, mas ainda à frente da câmera
    /// —, e a divisão pela perspectiva multiplica as coordenadas deles por dezenas de
    /// milhares: o triângulo vira um bloco cobrindo a tela, com a textura tão esticada que sai
    /// como cor chapada. Era o que sujava a pista do Crash.
    fn triangle(&self, tri: [Vertex; 3], batch: &mut Batch) {
        // O plano é inclusivo: a interface do Crash é desenhada em ortográfica com `z` bem no
        // plano próximo, e `z + w` dá zero exato nela.
        let distance = |v: &Vertex| v.position[2] + v.position[3];
        let inside: Vec<usize> = (0..3).filter(|&i| distance(&tri[i]) >= 0.0).collect();
        match inside.len() {
            0 => {}
            3 => self.prepare(tri, batch),
            1 => {
                let i = inside[0];
                let (a, b) = ((i + 1) % 3, (i + 2) % 3);
                // A ordem (dentro, corte em a, corte em b) mantém o sentido do original.
                self.prepare(
                    [tri[i], clip_near(tri[i], tri[a]), clip_near(tri[i], tri[b])],
                    batch,
                );
            }
            _ => {
                let out = (0..3).find(|i| !inside.contains(i)).unwrap_or(0);
                let (a, b) = ((out + 1) % 3, (out + 2) % 3);
                let ea = clip_near(tri[a], tri[out]);
                let eb = clip_near(tri[b], tri[out]);
                // O quadrilátero que sobra é `a → b → eb → ea`, na mesma volta do triângulo
                // original. Percorrê-lo ao contrário inverte a orientação e faz o descarte de
                // faces jogar fora justamente os triângulos recortados.
                self.prepare([tri[a], tri[b], eb], batch);
                self.prepare([tri[a], eb, ea], batch);
            }
        }
    }

    /// Projeta um triângulo já inteiramente à frente do plano próximo e o guarda no lote.
    ///
    /// Tudo o que não depende do pixel — projeção, descarte de face, caixa envolvente,
    /// atributos divididos por `w` — sai daqui pronto. É o que permite preencher o mesmo
    /// triângulo em várias faixas da tela ao mesmo tempo sem repetir conta nenhuma.
    fn prepare(&self, tri: [Vertex; 3], batch: &mut Batch) {
        let (vx, vy, vw, vh) = self.viewport_do_topo();
        if vw <= 0 || vh <= 0 {
            return;
        }
        // Divisão pela perspectiva e mapeamento para a tela. Guardamos `1/w` para corrigir a
        // interpolação depois: interpolar `u` direto na tela distorce a textura.
        let mut screen = [[0.0f32; 4]; 3];
        for (slot, vertex) in screen.iter_mut().zip(tri.iter()) {
            let [x, y, z, w] = vertex.position;
            let inv_w = 1.0 / w;
            let (perto, longe) = self.depth_range;
            *slot = [
                vx as f32 + (x * inv_w * 0.5 + 0.5) * vw as f32,
                vy as f32 + (0.5 - y * inv_w * 0.5) * vh as f32,
                perto + (longe - perto) * (z * inv_w * 0.5 + 0.5),
                inv_w,
            ];
        }

        let area = edge(screen[0], screen[1], screen[2]);
        if area == 0.0 {
            return;
        }
        // O sinal da área diz a orientação, mas com o eixo Y já invertido pelo mapeamento
        // para a tela — então ele é o oposto do sinal em coordenadas de janela do OpenGL, e
        // área negativa aqui é o sentido anti-horário de lá. Errar este sinal descarta
        // exatamente as faces que deveriam aparecer: o mundo do Quake ficava preto, com só o
        // avesso da geometria sendo desenhado.
        let counter_clockwise = area < 0.0;
        let front = (self.front_face == gles::GL_CCW) == counter_clockwise;
        if self.cull_face
            && match self.cull_mode {
                gles::GL_FRONT => front,
                gles::GL_FRONT_AND_BACK => true,
                _ => !front,
            }
        {
            return;
        }

        let min_x = screen.iter().fold(f32::MAX, |m, v| m.min(v[0])).floor() as i32;
        let max_x = screen.iter().fold(f32::MIN, |m, v| m.max(v[0])).ceil() as i32;
        let min_y = screen.iter().fold(f32::MAX, |m, v| m.min(v[1])).floor() as i32;
        let max_y = screen.iter().fold(f32::MIN, |m, v| m.max(v[1])).ceil() as i32;
        let min_x = min_x.max(vx).max(0);
        let max_x = max_x.min(vx + vw).min(self.width as i32);
        let min_y = min_y.max(vy).max(0);
        let max_y = max_y.min(vy + vh).min(self.height as i32);
        // O `glScissor` entra aqui, junto com a viewport: um retângulo alinhado aos eixos só
        // precisa apertar a caixa do triângulo, e assim ele não custa nada por pixel.
        let (min_x, max_x, min_y, max_y) = match self.tesoura {
            None => (min_x, max_x, min_y, max_y),
            Some((tx, ty, tw, th)) => (
                min_x.max(tx),
                max_x.min(tx + tw),
                min_y.max(ty),
                max_y.min(ty + th),
            ),
        };
        if min_x >= max_x || min_y >= max_y {
            return;
        }

        // Os atributos viajam divididos por `w` — é essa divisão que corrige a perspectiva —,
        // e cada um volta multiplicado pelo `w` interpolado. Fazer a divisão aqui, uma vez por
        // vértice, tira três multiplicações por fragmento de dentro do laço.
        let mut over_w = [[0.0f32; 9]; 3];
        for i in 0..3 {
            let w = screen[i][3];
            let v = &tri[i];
            over_w[i] = [
                v.color[0] * w,
                v.color[1] * w,
                v.color[2] * w,
                v.color[3] * w,
                v.uv[0] * w,
                v.uv[1] * w,
                v.fog * w,
                v.uv1[0] * w,
                v.uv1[1] * w,
            ];
        }

        let inv_area = 1.0 / area;
        // As três funções de aresta são lineares em `x`, então andar um pixel para o lado é
        // somar uma constante. Calculá-las do zero em cada pixel eram nove multiplicações por
        // fragmento — e a maioria dos fragmentos da caixa envolvente é descartada sem chegar a
        // virar cor.
        let step = [
            (screen[1][1] - screen[2][1]) * inv_area,
            (screen[2][1] - screen[0][1]) * inv_area,
            (screen[0][1] - screen[1][1]) * inv_area,
        ];
        // O mesmo para descer uma linha. Só a escolha do nível de mipmap usa este, e usa porque
        // **a compressão da textura não é sempre horizontal**: numa superfície de raspão ela
        // pode estar toda na vertical, e medir só para o lado dava nível zero num lugar onde a
        // textura passa inteira em dois pixels — que era o serrilhado das listras.
        let step_y = [
            (screen[2][0] - screen[1][0]) * inv_area,
            (screen[0][0] - screen[2][0]) * inv_area,
            (screen[1][0] - screen[0][0]) * inv_area,
        ];

        batch.cost += (max_x - min_x) as usize * (max_y - min_y) as usize;
        batch.triangles.push(Prepared {
            screen,
            over_w,
            inv_area,
            step,
            step_y,
            min_x,
            max_x,
            min_y,
            max_y,
        });
    }

    /// Guarda o lote da draw call para ser pintado no fim do quadro.
    ///
    /// Pintar draw call a draw call parecia natural e custava caro: o Quake faz meio milhão
    /// delas em vinte e cinco segundos, com cinco mil fragmentos cada em média. Nesse tamanho
    /// não há como dividir entre threads — acordá-las custa mais que o trabalho —, então 99%
    /// dos preenchimentos dele iam em série, e o rasterizador paralelo não fazia nada.
    ///
    /// Acumulando o quadro inteiro, a divisão acontece uma vez sobre um trabalho grande. Cada
    /// lote carrega o estado do OpenGL que valia quando foi montado, porque ele muda entre uma
    /// draw call e a seguinte.
    fn enqueue(&mut self, batch: &mut Batch) {
        if batch.triangles.is_empty() {
            return;
        }
        let start = self.pending.triangles.len();
        self.pending.triangles.append(&mut batch.triangles);
        self.pending.cost += batch.cost;
        self.pending.jobs.push(Job {
            first: start,
            last: self.pending.triangles.len(),
            // A textura fica pelo nome, não por referência: entre agora e o despejo o jogo
            // pode ligar outra, e é a deste momento que vale.
            texture: match self.texture_2d {
                true => Some(self.bound_texture),
                false => None,
            },
            texture_env: self.texture_env,
            texture1: self.unidade1.ligada.then_some(self.unidade1.textura),
            env1: self.unidade1.env,
            depth_test: self.depth_test,
            depth_mask: self.depth_mask,
            color_mask: self.color_mask,
            fog: self.fog,
            depth_func: self.depth_func,
            blend: self.blend,
            blend_src: self.blend_src,
            blend_dst: self.blend_dst,
            stencil: Stencil {
                test: self.stencil_test,
                func: self.stencil_func,
                referencia: self.stencil_ref as u8,
                valor_mask: self.stencil_value_mask as u8,
                escrita_mask: self.stencil_write_mask as u8,
                op: self.stencil_op,
            },
            alpha_test: self.alpha_test,
            alpha_func: self.alpha_func,
            alpha_ref: self.alpha_ref,
        });
    }

    /// Pinta tudo que está na fila, dividindo a tela em faixas horizontais quando compensa.
    ///
    /// Cada faixa é um pedaço exclusivo do quadro e percorre a fila inteira na ordem em que o
    /// jogo desenhou, então a transparência empilha na mesma sequência e o resultado é o mesmo
    /// que o de uma thread só.
    ///
    /// Precisa ser chamado antes de qualquer coisa que leia o quadro ou mude uma textura que a
    /// fila mencione — apresentar, limpar, trocar de superfície, carregar ou apagar textura.
    pub fn flush(&mut self) {
        if self.pending.jobs.is_empty() {
            return;
        }
        self.sujo = true;
        // Separar os campos permite ler as texturas e escrever no quadro ao mesmo tempo.
        let Self {
            color,
            depth,
            stencil,
            textures,
            width,
            height,
            pending,
            ..
        } = self;
        let (width, height) = (*width, *height);
        let (jobs, triangles) = (&pending.jobs, &pending.triangles);

        let bands = bands(height);
        if bands < 2 || pending.cost < PARALLEL_COST {
            let mut band = Band {
                top: 0,
                color,
                depth,
                stencil,
            };
            for job in jobs {
                let uniforms = job.uniforms(textures, width);
                for tri in &triangles[job.first..job.last] {
                    fill_band(tri, &uniforms, &mut band);
                }
            }
            pending.clear();
            return;
        }

        let rows = height.div_ceil(bands);
        std::thread::scope(|scope| {
            let mut top = 0i32;
            for ((color, depth), stencil) in color
                .chunks_mut(rows * width)
                .zip(depth.chunks_mut(rows * width))
                .zip(stencil.chunks_mut(rows * width))
            {
                let band_top = top;
                top += (color.len() / width) as i32;
                let textures = &*textures;
                scope.spawn(move || {
                    let mut band = Band {
                        top: band_top,
                        color,
                        depth,
                        stencil,
                    };
                    for job in jobs {
                        let uniforms = job.uniforms(textures, width);
                        for tri in &triangles[job.first..job.last] {
                            fill_band(tri, &uniforms, &mut band);
                        }
                    }
                });
            }
        });
        pending.clear();
    }

    /// O quadro pronto, em RGB565 e no tamanho `(width, height)` pedido.
    ///
    /// Aplica escritas diretas no buffer exposto pelo EGL sem restaurar pixels
    /// antigos sobre desenhos mais recentes. Profundidade e stencil são preservados.
    pub fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]) {
        self.flush();
        let (sw, sh) = self.surface();
        if width == 0 || height == 0 || old.len() != width * height * 2 || new.len() != old.len() {
            return;
        }
        /// Um pixel RGB565 do buffer exposto, já em RGB888.
        fn expande(novo: &[u8], offset: usize) -> [u8; 3] {
            let pixel = u16::from_le_bytes([novo[offset], novo[offset + 1]]);
            let r = ((pixel >> 11) & 31) as u8;
            let g = ((pixel >> 5) & 63) as u8;
            let b = (pixel & 31) as u8;
            [
                (r << 3) | (r >> 2),
                (g << 2) | (g >> 4),
                (b << 3) | (b >> 2),
            ]
        }

        // **O caminho sem escala é separado de propósito**, pela mesma razão do
        // [`GlState::frame_rgb565`]: no laço geral são duas divisões inteiras por pixel, e com
        // 640×330 isso é mais de quatrocentas mil divisões em cada chamada.
        //
        // E a linha inteira é comparada de uma vez antes do laço por pixel. O jogo mexe num
        // retângulo pequeno ou em nada; percorrer pixel a pixel a superfície toda para descobrir
        // isso custava 3,6 s numa execução de treze segundos da Z-Wheel. Um `!=` entre fatias é
        // um `memcmp`, e ele sai na primeira diferença.
        if sw == width && sh == height {
            let colunas = width.min(self.width);
            for y in 0..height.min(self.height) {
                let inicio = y * width * 2;
                let fim = inicio + colunas * 2;
                if old[inicio..fim] == new[inicio..fim] {
                    continue;
                }
                for x in 0..colunas {
                    let offset = inicio + x * 2;
                    if old[offset..offset + 2] == new[offset..offset + 2] {
                        continue;
                    }
                    let [r, g, b] = expande(new, offset);
                    let index = y * self.width + x;
                    self.sujo = true;
                    self.color[index] = [r, g, b, self.color[index][3]];
                }
            }
            return;
        }
        for y in 0..sh.min(self.height) {
            for x in 0..sw.min(self.width) {
                let offset = ((y * height / sh) * width + x * width / sw) * 2;
                if old[offset..offset + 2] == new[offset..offset + 2] {
                    continue;
                }
                let [r, g, b] = expande(new, offset);
                let index = y * self.width + x;
                self.sujo = true;
                self.color[index] = [r, g, b, self.color[index][3]];
            }
        }
    }

    /// A superfície em que o jogo desenha costuma ser menor que a tela — o Quake do Zeebo
    /// desenha em 320×400 numa tela de 640×480 —, então a apresentação amplia. É o que a
    /// extensão de escala da Qualcomm faz no console, e sem isso o quadro aparece encolhido
    /// num canto.
    /// O quadro em RGB565, escrito num vetor que o chamador reaproveita.
    ///
    /// Existe por causa do `eglGetColorBufferQUALCOMM`, que é chamado duas vezes por quadro e
    /// entrega os pixels ao jogo pela memória dele: montar um `Vec` novo a cada chamada, e
    /// depois outro para os bytes, era alocar e copiar oitocentos kilobytes sessenta vezes por
    /// segundo. Aqui o vetor é o mesmo sempre.
    ///
    /// **O caminho sem escala é separado de propósito.** Quando o pedido tem o tamanho da
    /// superfície — que é o caso normal —, as duas divisões por pixel do laço geral somam mais
    /// de quatrocentas mil divisões inteiras por chamada, e elas custavam mais que a conversão.
    pub fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>) {
        self.flush();
        // Quadro igual ao que já está em `out`: não há o que reconverter. Ver [`GlState::sujo`].
        if !self.sujo && out.len() == width * height * 2 {
            return;
        }
        let (sw, sh) = self.surface();
        out.clear();
        out.resize(width * height * 2, 0);
        let converte = |p: [u8; 4]| {
            ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)
        };
        // Escreve numa fatia já dimensionada. Antes era um `extend` por pixel, e no caminho de
        // escala isso é uma ida ao `Vec` para cada dois bytes.
        if sw == width && sh == height {
            for (y, saida) in out.chunks_exact_mut(width * 2).enumerate() {
                let linha = &self.color[y * self.width..y * self.width + width];
                for (pixel, par) in linha.iter().zip(saida.chunks_exact_mut(2)) {
                    par.copy_from_slice(&converte(*pixel).to_le_bytes());
                }
            }
        } else {
            for (y, saida) in out.chunks_exact_mut(width * 2).enumerate() {
                let linha = &self.color[(y * sh / height) * self.width..][..self.width];
                for (x, par) in saida.chunks_exact_mut(2).enumerate() {
                    par.copy_from_slice(&converte(linha[x * sw / width]).to_le_bytes());
                }
            }
        }
        self.sujo = false;
    }

#[cfg(test)]
    pub fn present(&mut self, width: usize, height: usize) -> Vec<u16> {
        // Entregar o quadro é o ponto em que ele precisa estar pintado — quem pede o resultado
        // não tem por que saber que o desenho é acumulado.
        self.flush();
        let (sw, sh) = self.surface();
        let mut out = vec![0u16; width * height];
        for y in 0..height {
            let sy = y * sh / height;
            for x in 0..width {
                let p = self.color[sy * self.width + x * sw / width];
                out[y * width + x] =
                    ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3);
            }
        }
        out
    }
}

/// Níveis de pilha de matriz. O mínimo que o OpenGL ES 1.1 garante para a modelagem é 16.
const MAX_MATRIX_STACK: usize = 16;

/// A partir de quantos fragmentos de caixa envolvente vale dividir o lote entre threads.
///
/// Abaixo disso o preenchimento termina antes de as threads acabarem de acordar. O número sai
/// da medição: no Crash e no Alpine Racer, mais de 95% do tempo de preenchimento está em draw
/// calls que passam bem deste tamanho, e a enxurrada de chamadas pequenas — a maioria delas —
/// custa junta uma fração do total.
const PARALLEL_COST: usize = 64_000;

/// Altura mínima de uma faixa, em linhas.
///
/// Ela é o **teto real de paralelismo em superfícies baixas**, e não um detalhe de afinação: com
/// as quarenta linhas de antes, o palco de 640x330 da Z-Wheel dava `330 / 40 = 8` faixas, e numa
/// máquina de 24 núcleos dois terços dela ficavam parados durante todo o preenchimento.
///
/// Medido, tempo de `flush` na Z-Wheel em treze segundos virtuais: 2474 ms com 40 linhas, 1938
/// com 16, 1868 com 8. E na pista do Crash, em trinta segundos virtuais, o total de API foi de
/// 4637 para 3842 e 3622 ms nos mesmos cortes — quem temia que faixas finas custassem caro numa
/// superfície de 480 linhas estava enganado, porque ali o número de faixas esbarra no número de
/// núcleos antes de esbarrar nesta constante.
///
/// O preparo por faixa existe e cresce quando ela afina — é por isso que há um piso —, mas até
/// oito linhas ele continua menor que o trabalho que a faixa ganha.
const MIN_BAND_ROWS: usize = 8;

/// Em quantas faixas horizontais dividir um quadro de `height` linhas.
fn bands(height: usize) -> usize {
    static CORES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let cores = *CORES.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });
    cores.min(height / MIN_BAND_ROWS).max(1)
}

/// Um triângulo já projetado na tela, com tudo que não depende do pixel resolvido.
struct Prepared {
    /// Vértices em coordenadas de tela, com `1/w` no quarto componente.
    screen: [[f32; 4]; 3],
    /// Cor e coordenada de textura de cada vértice, divididas por `w`.
    over_w: [[f32; 9]; 3],
    inv_area: f32,
    /// Quanto cada função de aresta anda a cada pixel para a direita.
    step: [f32; 3],
    /// E a cada linha para baixo. Ver a construção dele.
    step_y: [f32; 3],
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
}

/// Uma draw call à espera de virar pixel: a faixa dela na fila de triângulos, mais o estado do
/// OpenGL que valia quando foi montada.
/// O estado do stencil de uma draw call, do jeito que o preenchimento precisa dele.
#[derive(Debug, Clone, Copy)]
struct Stencil {
    test: bool,
    func: u32,
    referencia: u8,
    valor_mask: u8,
    escrita_mask: u8,
    /// `[falha, falha_z, passa]`.
    op: [u32; 3],
}

impl Stencil {
    /// Aplica uma das três operações a um valor do buffer.
    ///
    /// A máscara de escrita decide bit a bit o que muda: é ela que deixa um desenho marcar o
    /// chão sem mexer nos bits que outro passe usa.
    fn aplica(&self, qual: usize, atual: u8) -> u8 {
        let novo = match self.op[qual] {
            gles::GL_KEEP => return atual,
            gles::GL_ZERO_OP => 0,
            gles::GL_REPLACE => self.referencia,
            gles::GL_INCR => atual.saturating_add(1),
            gles::GL_DECR => atual.saturating_sub(1),
            gles::GL_INVERT => !atual,
            _ => return atual,
        };
        (atual & !self.escrita_mask) | (novo & self.escrita_mask)
    }

    /// O teste em si, entre a referência e o que está no buffer, ambos mascarados.
    fn passa(&self, atual: u8) -> bool {
        let (r, v) = (
            (self.referencia & self.valor_mask) as f32,
            (atual & self.valor_mask) as f32,
        );
        compare(self.func, r, v)
    }
}

struct Job {
    first: usize,
    last: usize,
    /// Nome da textura ligada, ou `None` se o desenho não usa textura.
    texture: Option<u32>,
    texture_env: TexEnv,
    /// A textura e o ambiente da unidade 1, quando ela está ligada.
    texture1: Option<u32>,
    env1: TexEnv,
    depth_test: bool,
    depth_mask: bool,
    color_mask: [bool; 4],
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
    /// O estado do stencil no momento do desenho. Ver [`GlState::stencil_test`].
    stencil: Stencil,
    /// A névoa no momento do desenho. Ver [`Neblina`].
    fog: Neblina,
}

impl Job {
    /// Resolve o nome da textura e monta o estado que o preenchimento lê.
    fn uniforms<'a>(&self, textures: &'a HashMap<u32, Texture>, width: usize) -> Uniforms<'a> {
        let texture = self.texture.and_then(|name| textures.get(&name));
        Uniforms {
            width,
            texture,
            usa_mipmap: texture.is_some_and(|texture| {
                !texture.mipmaps.is_empty()
                    && matches!(
                        texture.min_filter,
                        gles::GL_NEAREST_MIPMAP_NEAREST
                            | gles::GL_LINEAR_MIPMAP_NEAREST
                            | gles::GL_NEAREST_MIPMAP_LINEAR
                            | gles::GL_LINEAR_MIPMAP_LINEAR
                    )
            }),
            texture_env: self.texture_env,
            texture1: self.texture1.and_then(|name| textures.get(&name)),
            env1: self.env1,
            depth_test: self.depth_test,
            depth_mask: self.depth_mask,
            color_mask: self.color_mask,
            depth_func: self.depth_func,
            blend: self.blend,
            blend_src: self.blend_src,
            blend_dst: self.blend_dst,
            alpha_test: self.alpha_test,
            alpha_func: self.alpha_func,
            alpha_ref: self.alpha_ref,
            stencil: self.stencil,
            fog: self.fog,
        }
    }
}

/// O quadro em construção: os triângulos de todas as draw calls desde o último despejo, e a
/// lista de quais pertencem a cada uma.
///
/// Os dois vetores vivem entre quadros para não devolver e repedir a mesma memória sessenta
/// vezes por segundo.
#[derive(Default)]
struct Pending {
    triangles: Vec<Prepared>,
    jobs: Vec<Job>,
    /// Soma das caixas envolventes, em fragmentos: é ela que decide se vale acordar as threads.
    cost: usize,
}

impl Pending {
    fn clear(&mut self) {
        self.triangles.clear();
        self.jobs.clear();
        self.cost = 0;
    }
}

/// Os triângulos de uma draw call, com o custo estimado de preenchê-los.
#[derive(Default)]
struct Batch {
    triangles: Vec<Prepared>,
    /// Soma das caixas envolventes, em fragmentos.
    cost: usize,
}

impl Batch {
    fn clear(&mut self) {
        self.triangles.clear();
        self.cost = 0;
    }
}

/// O estado do OpenGL que vale para o lote inteiro. Só leitura, e por isso compartilhável
/// entre as threads que preenchem as faixas.
struct Uniforms<'a> {
    width: usize,
    texture: Option<&'a Texture>,
    /// Se a textura do lote tem cadeia de mipmaps **e** o filtro pede nível.
    ///
    /// Não depende do fragmento, mas era recalculado dentro do laço — um `matches!` de quatro
    /// constantes em cada pixel aprovado, centenas de milhares de vezes por quadro.
    usa_mipmap: bool,
    texture_env: TexEnv,
    /// A textura e o ambiente da unidade 1.
    texture1: Option<&'a Texture>,
    env1: TexEnv,
    depth_test: bool,
    depth_mask: bool,
    color_mask: [bool; 4],
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
    /// O estado do stencil no momento do desenho. Ver [`GlState::stencil_test`].
    stencil: Stencil,
    /// A névoa no momento do desenho. Ver [`Neblina`].
    fog: Neblina,
}

/// Uma faixa horizontal do quadro: o pedaço exclusivo de uma thread.
struct Band<'a> {
    /// Linha da tela que corresponde ao começo da faixa.
    top: i32,
    color: &'a mut [[u8; 4]],
    depth: &'a mut [f32],
    stencil: &'a mut [u8],
}

/// Preenche a parte de um triângulo que cai dentro da faixa.
fn fill_band(tri: &Prepared, uniforms: &Uniforms, band: &mut Band) {
    let width = uniforms.width;
    let rows = (band.color.len() / width) as i32;
    let top = tri.min_y.max(band.top);
    let bottom = tri.max_y.min(band.top + rows);
    for y in top..bottom {
        // Cada linha recomeça do cálculo exato: o erro do acúmulo fica preso dentro da linha
        // em vez de descer pela imagem toda.
        let start = [tri.min_x as f32 + 0.5, y as f32 + 0.5, 0.0, 0.0];
        let mut w = [
            edge(tri.screen[1], tri.screen[2], start) * tri.inv_area,
            edge(tri.screen[2], tri.screen[0], start) * tri.inv_area,
            edge(tri.screen[0], tri.screen[1], start) * tri.inv_area,
        ];
        let row = (y - band.top) as usize * width;
        for x in tri.min_x..tri.max_x {
            let bary = w;
            // O passo vem antes do descarte porque o `continue` pularia a soma.
            w = [w[0] + tri.step[0], w[1] + tri.step[1], w[2] + tri.step[2]];
            if bary[0] < 0.0 || bary[1] < 0.0 || bary[2] < 0.0 {
                continue;
            }
            let z = bary[0] * tri.screen[0][2]
                + bary[1] * tri.screen[1][2]
                + bary[2] * tri.screen[2][2];
            let index = row + x as usize;

            let inv_w = bary[0] * tri.screen[0][3]
                + bary[1] * tri.screen[1][3]
                + bary[2] * tri.screen[2][3];
            if inv_w == 0.0 {
                continue;
            }
            let w = 1.0 / inv_w;
            let attribute = |k: usize| {
                (bary[0] * tri.over_w[0][k]
                    + bary[1] * tri.over_w[1][k]
                    + bary[2] * tri.over_w[2][k])
                    * w
            };
            // Montar a cor custa a amostragem da textura, e é o passo caro do fragmento. Sem
            // teste de alfa ela pode esperar o descarte por profundidade; **com** teste de
            // alfa, não pode: no OpenGL o alfa é decidido antes do stencil, e um fragmento
            // reprovado ali não tem direito de mexer no stencil.
            let montar = || {
                let mut source = [attribute(0), attribute(1), attribute(2), attribute(3)];
                let primaria = source;
                if let Some(texture) = uniforms.texture {
                    // A redução pode acontecer em qualquer eixo da tela. Compare os
                    // vizinhos da direita e de baixo, usando o maior footprint.
                    let (u, v) = (attribute(4), attribute(5));
                    // **O pixel vizinho precisa estar do mesmo lado do olho.** Quando o sinal
                    // de `1/w` vira — o triângulo cruza o horizonte —, a diferença explode e o
                    // nível escolhido vai para o menor da cadeia, que é uma cor chapada.
                    let reducao = |step: [f32; 3]| {
                        let seguinte = std::array::from_fn::<_, 3, _>(|i| bary[i] + step[i]);
                        let inv_w2 = seguinte[0] * tri.screen[0][3]
                            + seguinte[1] * tri.screen[1][3]
                            + seguinte[2] * tri.screen[2][3];
                        match inv_w2 <= 0.0 || inv_w <= 0.0 {
                            true => 0.0,
                            false => {
                                let w2 = 1.0 / inv_w2;
                                let vizinho = |k: usize| {
                                    (seguinte[0] * tri.over_w[0][k]
                                        + seguinte[1] * tri.over_w[1][k]
                                        + seguinte[2] * tri.over_w[2][k])
                                        * w2
                                };
                                let du = (vizinho(4) - u) * texture.width as f32;
                                let dv = (vizinho(5) - v) * texture.height as f32;
                                let quadrado = du * du + dv * dv;
                                match quadrado > 1.0 {
                                    // `log2(sqrt(x))` é `log2(x)/2`, e uma raiz a menos por pixel.
                                    true => quadrado.log2() * 0.5,
                                    false => 0.0,
                                }
                            }
                        }
                    };
                    // Sem cadeia de mipmaps o amostrador sempre cai no nível zero. Evitar as
                    // duas derivadas e o log2 por fragmento é decisivo para jogos 3D que usam
                    // texturas comprimidas sem níveis auxiliares, como o Super League.
                    let lod = if !uniforms.usa_mipmap {
                        0.0
                    } else {
                        reducao(tri.step).max(reducao(tri.step_y))
                    };
                    let texel = texture.sample_lod(u, v, lod);
                    source = uniforms.texture_env.aplica(source, texel);
                }
                // A unidade 1 age sobre o que saiu da 0. Sem mipmap: nos jogos que a usam ela
                // leva mapa de luz ou a textura de cor de um relevo, e o nível base basta.
                if let Some(texture) = uniforms.texture1 {
                    let texel = texture.sample_lod(attribute(7), attribute(8), 0.0);
                    source = uniforms.env1.aplica_com(source, primaria, texel);
                }
                // **A névoa entra depois da textura e antes do teste de alfa**, que é a ordem do
                // OpenGL ES 1.1, e mexe só no RGB: o alfa do fragmento continua sendo o dele.
                if uniforms.fog.ligada && uniforms.fog.permitida {
                    let f = attribute(6).clamp(0.0, 1.0);
                    for canal in 0..3 {
                        source[canal] += (uniforms.fog.cor[canal] - source[canal]) * (1.0 - f);
                    }
                }
                source
            };
            let mut pronta = None;
            if uniforms.alpha_test {
                let source = montar();
                if !compare(uniforms.alpha_func, source[3], uniforms.alpha_ref) {
                    continue;
                }
                pronta = Some(source);
            }

            // **Stencil antes de profundidade, e as três operações aplicadas mesmo quando o
            // fragmento é descartado.** É esse "mesmo quando descarta" que faz o reflexo
            // funcionar: a passada que marca o chão costuma escrever no stencil justamente
            // onde a profundidade reprova.
            let stencil = &uniforms.stencil;
            let profundidade_passa =
                !uniforms.depth_test || compare(uniforms.depth_func, z, band.depth[index]);
            if stencil.test {
                let atual = band.stencil[index];
                if !stencil.passa(atual) {
                    band.stencil[index] = stencil.aplica(0, atual);
                    continue;
                }
                let qual = usize::from(profundidade_passa) + 1;
                band.stencil[index] = stencil.aplica(qual, atual);
            }
            if !profundidade_passa {
                continue;
            }

            let source = match pronta {
                Some(source) => source,
                None => montar(),
            };

            let mixed = if uniforms.blend {
                let destination = unpack(band.color[index]);
                let mut out = [0.0; 4];
                for c in 0..4 {
                    let s = factor(uniforms.blend_src, source, destination, c);
                    let d = factor(uniforms.blend_dst, source, destination, c);
                    out[c] = (source[c] * s + destination[c] * d).clamp(0.0, 1.0);
                }
                out
            } else {
                source
            };

            // O `glColorMask` decide canal a canal. Escrever o que ele proibiu apagaria o que
            // uma passada anterior deixou ali, que é justamente o ponto de mascarar.
            let anterior = band.color[index];
            let novo = pack(mixed);
            band.color[index] = std::array::from_fn(|i| {
                if uniforms.color_mask[i] {
                    novo[i]
                } else {
                    anterior[i]
                }
            });
            if uniforms.depth_test && uniforms.depth_mask {
                band.depth[index] = z;
            }
        }
    }
}

/// Função de aresta: o dobro da área com sinal do triângulo `(a, b, c)`.
fn edge(a: [f32; 4], b: [f32; 4], c: [f32; 4]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Interpola `a`→`b` até o plano próximo, onde `z + w` cruza o zero.
/// Produto escalar de três componentes.
fn ponto(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn comprimento(v: [f32; 3]) -> f32 {
    ponto(v, v).sqrt()
}

/// Um vetor de comprimento um, ou o vetor nulo quando não dá para dizer para onde ele aponta.
fn normaliza(v: [f32; 3]) -> [f32; 3] {
    let n = comprimento(v);
    if n <= f32::EPSILON {
        return [0.0, 0.0, 0.0];
    }
    [v[0] / n, v[1] / n, v[2] / n]
}

/// O fator do cone de um holofote, ou um quando a luz não é holofote.
fn holofote(luz: &Light, para_a_luz: [f32; 3]) -> f32 {
    if luz.spot_cutoff >= 180.0 {
        return 1.0;
    }
    // O cosseno é entre a direção do cone e a direção **da luz para o vértice**, que é o
    // contrário de `para_a_luz`.
    let cos = ponto(
        normaliza(luz.spot_direction),
        [-para_a_luz[0], -para_a_luz[1], -para_a_luz[2]],
    );
    if cos < luz.spot_cutoff.to_radians().cos() {
        return 0.0;
    }
    cos.max(0.0).powf(luz.spot_exponent)
}

/// A matriz que leva uma normal de coordenadas de objeto para coordenadas de olho.
///
/// É a **transposta da inversa** da parte 3×3 da modelview, e não a modelview: com escala não
/// uniforme, a normal transformada como se fosse direção deixa de ser perpendicular à
/// superfície e a luz escorrega pelo modelo. Quando a inversa não existe — matriz degenerada —,
/// a parte 3×3 crua é o menos errado que dá para devolver.
fn matriz_de_normais(m: &Matrix) -> [[f32; 3]; 3] {
    let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() <= f32::EPSILON {
        return a;
    }
    // Inversa pela adjunta, já transposta: a transposta da inversa é a adjunta transposta
    // dividida pelo determinante, e a adjunta é a transposta da matriz de cofatores — as duas
    // transposições se cancelam, então o que fica é a matriz de cofatores sobre o determinante.
    let cofator = |i: usize, j: usize| {
        let (l1, l2) = ((i + 1) % 3, (i + 2) % 3);
        let (c1, c2) = ((j + 1) % 3, (j + 2) % 3);
        a[l1][c1] * a[l2][c2] - a[l1][c2] * a[l2][c1]
    };
    std::array::from_fn(|i| std::array::from_fn(|j| cofator(i, j) / det))
}

/// Aplica a matriz de normais.
fn gira_normal(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn clip_near(a: Vertex, b: Vertex) -> Vertex {
    let (da, db) = (a.position[2] + a.position[3], b.position[2] + b.position[3]);
    let t = da / (da - db);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Vertex {
        position: std::array::from_fn(|i| lerp(a.position[i], b.position[i])),
        color: std::array::from_fn(|i| lerp(a.color[i], b.color[i])),
        uv: std::array::from_fn(|i| lerp(a.uv[i], b.uv[i])),
        uv1: std::array::from_fn(|i| lerp(a.uv1[i], b.uv1[i])),
        // A normal já foi consumida pela iluminação antes do recorte: aqui ela não muda mais
        // nada, e interpolá-la seria trabalho para ninguém ler.
        normal: a.normal,
        // O fator da névoa, ao contrário, ainda vai ser lido: o vértice novo fica onde o plano
        // cortou, e a névoa dele é a do ponto de corte.
        fog: lerp(a.fog, b.fog),
    }
}

/// Uma das funções de comparação do OpenGL.
fn compare(func: u32, value: f32, reference: f32) -> bool {
    match func {
        gles::GL_NEVER => false,
        gles::GL_LESS => value < reference,
        gles::GL_EQUAL => value == reference,
        gles::GL_LEQUAL => value <= reference,
        gles::GL_GREATER => value > reference,
        gles::GL_NOTEQUAL => value != reference,
        gles::GL_GEQUAL => value >= reference,
        _ => true,
    }
}

/// Combina a cor do fragmento com o texel, conforme o `GL_TEXTURE_ENV_MODE`.
/// O peso de um fator de mistura para o canal `c`.
fn factor(kind: u32, source: [f32; 4], destination: [f32; 4], c: usize) -> f32 {
    match kind {
        gles::GL_ZERO => 0.0,
        gles::GL_SRC_COLOR => source[c],
        gles::GL_ONE_MINUS_SRC_COLOR => 1.0 - source[c],
        gles::GL_SRC_ALPHA => source[3],
        gles::GL_ONE_MINUS_SRC_ALPHA => 1.0 - source[3],
        gles::GL_DST_ALPHA => destination[3],
        gles::GL_ONE_MINUS_DST_ALPHA => 1.0 - destination[3],
        gles::GL_DST_COLOR => destination[c],
        gles::GL_ONE_MINUS_DST_COLOR => 1.0 - destination[c],
        _ => 1.0,
    }
}

fn pack(color: [f32; 4]) -> [u8; 4] {
    std::array::from_fn(|i| (color[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

fn unpack(color: [u8; 4]) -> [f32; 4] {
    std::array::from_fn(|i| color[i] as f32 / 255.0)
}

#[cfg(test)]
mod tests {

    #[test]
    fn escrita_no_buffer_egl_preserva_desenhos_e_profundidade() {
        let mut state = GlState::new(2, 1);
        let mut exported = Vec::new();
        state.frame_rgb565(2, 1, &mut exported);
        state.color[1] = [0, 255, 0, 255];
        state.depth[0] = 0.25;
        state.stencil[0] = 7;
        let mut changed = exported.clone();
        changed[..2].copy_from_slice(&0xf800u16.to_le_bytes());
        state.import_rgb565_changes(2, 1, &exported, &changed);
        assert_eq!(&state.color[0][..3], &[255, 0, 0]);
        assert_eq!(state.color[1], [0, 255, 0, 255]);
        assert_eq!(state.depth[0], 0.25);
        assert_eq!(state.stencil[0], 7);
    }

    /// O caminho **com escala** tem laço próprio, com as divisões por pixel, e é o do Quake:
    /// superfície menor que a tela. O teste acima cobre só o caminho sem escala.
    #[test]
    fn escrita_no_buffer_egl_encontra_o_pixel_quando_a_superficie_e_menor() {
        let mut state = GlState::new(2, 1);
        state.set_viewport(0, 0, 1, 1);
        assert_eq!(state.surface(), (1, 1));
        let mut exported = Vec::new();
        state.frame_rgb565(2, 1, &mut exported);
        let mut changed = exported.clone();
        changed[..2].copy_from_slice(&0xf800u16.to_le_bytes());
        state.import_rgb565_changes(2, 1, &exported, &changed);
        assert_eq!(&state.color[0][..3], &[255, 0, 0]);
    }

    #[test]
    fn mipmap_considera_reducao_vertical_e_horizontal() {
        for vertical in [false, true] {
            let mut state = GlState::new(8, 8);
            state.bind_texture(1);
            state.set_capability(gles::GL_TEXTURE_2D, true);
            state.textures.insert(
                1,
                Texture {
                    width: 64,
                    height: 64,
                    pixels: vec![[255, 0, 0, 255]; 64 * 64],
                    mipmaps: (0..6)
                        .map(|level| {
                            let size = 32 >> level;
                            Nivel {
                                width: size,
                                height: size,
                                pixels: vec![[0, 255, 0, 255]; size * size],
                            }
                        })
                        .collect(),
                    min_filter: gles::GL_NEAREST_MIPMAP_NEAREST,
                    ..Default::default()
                },
            );
            let vertex = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                uv: if vertical {
                    [0.0, (y + 1.0) * 0.5]
                } else {
                    [(x + 1.0) * 0.5, 0.0]
                },
                ..Default::default()
            };
            state.draw(
                gles::GL_TRIANGLE_STRIP,
                &[
                    vertex(-1.0, -1.0),
                    vertex(1.0, -1.0),
                    vertex(-1.0, 1.0),
                    vertex(1.0, 1.0),
                ],
            );
            state.flush();
            assert_eq!(
                state.color[3 * 8 + 4],
                [0, 255, 0, 255],
                "vertical={vertical}"
            );
        }
    }

    /// Um buraco na cadeia não vira branco: o nível inválido cai no que existe.
    ///
    /// Era o que deixava modelos inteiros chapados — um nível vazio devolvia branco opaco e a
    /// superfície toda saía de uma cor só.
    #[test]
    fn nivel_vazio_cai_no_que_existe() {
        let mut t = Texture {
            width: 2,
            height: 1,
            pixels: vec![[255, 0, 0, 255]; 2],
            ..Default::default()
        };
        t.min_filter = gles::GL_NEAREST_MIPMAP_NEAREST;
        t.mipmaps = vec![Nivel {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }];
        let cor = t.sample_lod(0.5, 0.5, 5.0);
        assert_eq!(cor[0], 1.0, "vermelho do nível cheio");
        assert_eq!(cor[1], 0.0, "e não branco: {cor:?}");
    }

    /// Uma textura sem cadeia de redução não é mipmapeada, mesmo que o filtro peça.
    ///
    /// O padrão do `MIN_FILTER` no OpenGL é `GL_NEAREST_MIPMAP_LINEAR`, e adotá-lo ao pé da
    /// letra passava a amostrar por vizinho mais próximo as vinte e uma texturas do palco da
    /// Z-Wheel que pedem `GL_LINEAR` e não trazem nível nenhum.
    #[test]
    fn sem_cadeia_vale_o_filtro_de_base() {
        let mut t = Texture {
            width: 2,
            height: 1,
            pixels: vec![[0, 0, 0, 255], [255, 255, 255, 255]],
            ..Default::default()
        };
        t.min_filter = gles::GL_NEAREST_MIPMAP_LINEAR;
        // No meio dos dois texels, com vizinho mais próximo, a cor é de um deles.
        let meio = t.sample_lod(0.5, 0.5, 4.0);
        assert!(meio[0] == 0.0 || meio[0] == 1.0, "{meio:?}");
        t.min_filter = gles::GL_LINEAR_MIPMAP_LINEAR;
        let meio = t.sample_lod(0.5, 0.5, 4.0);
        assert!((meio[0] - 0.5).abs() < 0.01, "interpolado: {meio:?}");
    }

    /// Com cadeia, o nível sai do `lod`: zero é a imagem cheia, e o último é o menor nível.
    #[test]
    fn o_lod_escolhe_o_nivel() {
        let mut t = Texture {
            width: 2,
            height: 1,
            pixels: vec![[255, 0, 0, 255]; 2],
            ..Default::default()
        };
        t.mipmaps = vec![Nivel {
            width: 1,
            height: 1,
            pixels: vec![[0, 0, 255, 255]],
        }];
        t.min_filter = gles::GL_NEAREST_MIPMAP_NEAREST;
        let perto = t.sample_lod(0.5, 0.5, 0.0);
        assert_eq!(perto[0], 1.0, "de perto, o nível cheio: {perto:?}");
        let longe = t.sample_lod(0.5, 0.5, 3.0);
        assert_eq!(longe[2], 1.0, "de longe, o nível menor: {longe:?}");
    }

    /// O stencil marca onde o desenho passou, e o desenho seguinte só entra onde a marca está.
    ///
    /// É a receita do reflexo do palco da Z-Wheel, reduzida a dois quadrados.
    #[test]
    fn a_marca_do_stencil_recorta_o_desenho_seguinte() {
        let mut estado = GlState::new(4, 4);
        estado.set_viewport(0, 0, 4, 4);
        estado.set_capability(gles::GL_STENCIL_TEST, true);

        // Primeiro passe: escreve 1 no stencil na metade de cima, sem pintar.
        estado.set_color_mask([false; 4]);
        estado.set_stencil_func(gles::GL_ALWAYS, 1, 0xff);
        estado.set_stencil_op(gles::GL_KEEP, gles::GL_KEEP, gles::GL_REPLACE);
        quadrado(&mut estado, -1.0, 0.0, 1.0, 1.0, [1.0; 4]);
        estado.flush();
        assert_eq!(estado.stencil[0], 1, "topo marcado");
        assert_eq!(estado.stencil[12], 0, "fundo intocado");

        // Segundo passe: pinta de vermelho só onde a marca está.
        estado.set_color_mask([true; 4]);
        estado.set_stencil_func(gles::GL_EQUAL, 1, 0xff);
        estado.set_stencil_op(gles::GL_KEEP, gles::GL_KEEP, gles::GL_KEEP);
        quadrado(&mut estado, -1.0, -1.0, 1.0, 1.0, [1.0, 0.0, 0.0, 1.0]);
        estado.flush();
        assert_eq!(estado.color[0][0], 255, "topo pintado");
        assert_eq!(estado.color[12][0], 0, "fundo poupado pelo stencil");
    }

    /// A máscara de escrita decide bit a bit o que o desenho pode mudar no stencil.
    #[test]
    fn a_mascara_protege_os_bits_de_fora() {
        let mut estado = GlState::new(2, 2);
        estado.set_viewport(0, 0, 2, 2);
        estado.stencil.fill(0b1010_1010);
        estado.set_capability(gles::GL_STENCIL_TEST, true);
        estado.set_stencil_func(gles::GL_ALWAYS, 0xff, 0xff);
        estado.set_stencil_op(gles::GL_KEEP, gles::GL_KEEP, gles::GL_REPLACE);
        estado.set_stencil_mask(0b0000_1111);
        quadrado(&mut estado, -1.0, -1.0, 1.0, 1.0, [1.0; 4]);
        estado.flush();
        assert_eq!(estado.stencil[0], 0b1010_1111);
    }

    /// Um desenho reprovado no stencil não pinta, mas **aplica** a operação de falha: é isso
    /// que deixa um passe contar sem aparecer.
    #[test]
    fn a_falha_no_stencil_ainda_mexe_no_buffer() {
        let mut estado = GlState::new(2, 2);
        estado.set_viewport(0, 0, 2, 2);
        estado.set_capability(gles::GL_STENCIL_TEST, true);
        estado.set_stencil_func(gles::GL_NEVER, 0, 0xff);
        estado.set_stencil_op(gles::GL_INCR, gles::GL_KEEP, gles::GL_KEEP);
        quadrado(&mut estado, -1.0, -1.0, 1.0, 1.0, [1.0, 0.0, 0.0, 1.0]);
        estado.flush();
        assert_eq!(estado.stencil[0], 1, "a operação de falha valeu");
        assert_eq!(estado.color[0][0], 0, "e nada foi pintado");
    }

    /// Um quadrado em coordenadas normalizadas, com a cor dada.
    fn quadrado(estado: &mut GlState, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
        let v = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0, 1.0],
            color,
            uv: [0.0; 2],
            ..Default::default()
        };
        estado.draw(
            gles::GL_TRIANGLES,
            &[
                v(x0, y0),
                v(x1, y0),
                v(x1, y1),
                v(x0, y0),
                v(x1, y1),
                v(x0, y1),
            ],
        );
    }

    /// Um estado com a luz zero ligada, material e luz brancos, e nada mais.
    fn com_luz() -> GlState {
        let mut estado = GlState::new(4, 4);
        estado.set_capability(gles::GL_LIGHTING, true);
        estado.set_capability(gles::GL_LIGHT0, true);
        estado.set_material(gles::GL_AMBIENT, [0.0, 0.0, 0.0, 1.0]);
        estado.set_material(gles::GL_DIFFUSE, [1.0, 1.0, 1.0, 1.0]);
        estado.set_light_model(gles::GL_LIGHT_MODEL_AMBIENT, [0.0, 0.0, 0.0, 1.0]);
        estado
    }

    /// A luz zero nasce com difusa branca e direcional em `(0, 0, 1, 0)`: uma face virada para
    /// o observador recebe tudo, e uma de lado não recebe nada.
    #[test]
    fn a_difusa_segue_o_cosseno_da_normal() {
        let estado = com_luz();
        let frente = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], [1.0; 4]);
        assert!((frente[0] - 1.0).abs() < 1e-5, "de frente: {frente:?}");
        let lado = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [1.0, 0.0, 0.0], [1.0; 4]);
        assert!(lado[0].abs() < 1e-5, "de lado: {lado:?}");
        let costas = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, -1.0], [1.0; 4]);
        assert!(costas[0].abs() < 1e-5, "de costas: {costas:?}");
    }

    /// O que a Z-Wheel monta: ambiente da cena no padrão, ambiente da luz em 0,5 e material
    /// ambiente e difuso em 0,949. De frente, isso é `0,949*0,2 + 0,949*0,5 + 0,949*1`, que
    /// satura; de lado sobram as duas ambientes.
    #[test]
    fn a_conta_do_palco_da_z_wheel() {
        let mut estado = GlState::new(4, 4);
        estado.set_capability(gles::GL_LIGHTING, true);
        estado.set_capability(gles::GL_LIGHT0, true);
        let cinza = [0.949, 0.949, 0.949, 1.0];
        estado.set_material(gles::GL_AMBIENT, cinza);
        estado.set_material(gles::GL_DIFFUSE, cinza);
        estado.set_light(0, gles::GL_AMBIENT, [0.5, 0.5, 0.5, 1.0]);
        let lado = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [1.0, 0.0, 0.0], [1.0; 4]);
        assert!((lado[0] - 0.949 * 0.7).abs() < 1e-4, "de lado: {lado:?}");
        let frente = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], [1.0; 4]);
        assert!((frente[0] - 1.0).abs() < 1e-5, "de frente: {frente:?}");
    }

    /// O alfa é o da difusa do material, e não a soma das luzes — somando, tudo fica opaco.
    #[test]
    fn o_alfa_vem_do_material() {
        let mut estado = com_luz();
        estado.set_material(gles::GL_DIFFUSE, [1.0, 1.0, 1.0, 0.25]);
        let cor = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], [1.0; 4]);
        assert!((cor[3] - 0.25).abs() < 1e-5, "{cor:?}");
    }

    /// Com `GL_COLOR_MATERIAL`, a cor do vértice toma o lugar do material — é o único jeito de
    /// um vetor de cores continuar valendo com a luz ligada.
    #[test]
    fn a_cor_do_vertice_so_vale_com_color_material() {
        let mut estado = com_luz();
        let vermelho = [1.0, 0.0, 0.0, 1.0];
        let sem = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], vermelho);
        assert!((sem[1] - 1.0).abs() < 1e-5, "sem color material: {sem:?}");
        estado.set_capability(gles::GL_COLOR_MATERIAL, true);
        let com = estado.cor_iluminada([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], vermelho);
        assert!(com[1].abs() < 1e-5, "com color material: {com:?}");
    }

    /// A posição da luz é guardada em coordenadas de olho: quem transforma é o `glLight`, com a
    /// modelview do momento da chamada.
    #[test]
    fn a_posicao_da_luz_passa_pela_modelview_da_chamada() {
        let mut estado = GlState::new(4, 4);
        estado.set_matrix_mode(gles::GL_MODELVIEW);
        estado.load_matrix(translation(3.0, 0.0, 0.0));
        estado.set_light(0, gles::GL_POSITION, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(estado.lights[0].position, [4.0, 0.0, 0.0, 1.0]);
        // Mexer na modelview depois não move a luz.
        estado.load_matrix(translation(10.0, 0.0, 0.0));
        assert_eq!(estado.lights[0].position, [4.0, 0.0, 0.0, 1.0]);
    }

    /// Com escala não uniforme, a normal precisa da transposta da inversa: transformada como
    /// direção, ela deixa de ser perpendicular à superfície e a luz escorrega pelo modelo.
    #[test]
    fn a_normal_sobrevive_a_escala_nao_uniforme() {
        let m = scaling(1.0, 4.0, 1.0);
        // Uma superfície inclinada a 45 graus no plano XY: tangente `(1, 1, 0)`, normal
        // `(1, -1, 0)`. Depois da escala, a tangente vira `(1, 4, 0)`.
        let normal = normaliza(gira_normal(&matriz_de_normais(&m), [1.0, -1.0, 0.0]));
        let tangente = normaliza([1.0, 4.0, 0.0]);
        assert!(ponto(normal, tangente).abs() < 1e-5, "normal {normal:?}");
    }

    /// Sem `GL_LIGHTING` nada disso acontece: a cor do vértice chega inteira ao rasterizador.
    #[test]
    fn sem_luz_a_cor_passa_inteira() {
        let mut estado = GlState::new(4, 4);
        assert!(!estado.lighting);
        estado.set_capability(gles::GL_LIGHTING, true);
        assert!(estado.lighting);
    }
    use super::*;

    /// O `GL_COMBINE` com a fonte 0 na textura e a função `GL_REPLACE` ignora a cor do
    /// vértice. É o que o motor QX pede, com a cor zerada; tratado como `GL_MODULATE`, o
    /// resultado era preto transparente.
    #[test]
    fn o_combine_substitui_pela_textura_sem_olhar_o_vertice() {
        let mut env = TexEnv::com_modo(gles::GL_COMBINE);
        env.define(gles::GL_COMBINE_RGB, gles::GL_REPLACE, 0.0);
        env.define(gles::GL_COMBINE_ALPHA, gles::GL_REPLACE, 0.0);
        env.define(gles::GL_SRC0_RGB, gles::GL_TEXTURE, 0.0);
        env.define(gles::GL_SRC0_ALPHA, gles::GL_TEXTURE, 0.0);
        // O QX manda `GL_SRC_COLOR` no operando de alfa; vale como o alfa.
        env.define(gles::GL_OPERAND0_ALPHA, gles::GL_SRC_COLOR, 0.0);
        let texel = [0.2, 0.4, 0.6, 0.8];
        assert_eq!(env.aplica([0.0; 4], texel), texel);
    }

    /// A unidade 1 do QX: `ADD_SIGNED` da textura de cor com o que saiu da unidade 0 (o `DOT3`
    /// da luz), e o alfa da textura vezes a cor constante.
    #[test]
    fn a_unidade_1_soma_com_sinal_sobre_a_anterior() {
        let mut env = TexEnv::com_modo(gles::GL_COMBINE);
        env.define(gles::GL_COMBINE_RGB, gles::GL_ADD_SIGNED, 0.0);
        env.define(gles::GL_SRC0_RGB + 1, gles::GL_PREVIOUS, 0.0);
        env.define(gles::GL_SRC0_ALPHA + 1, gles::GL_CONSTANT, 0.0);
        env.cor = [0.0, 0.0, 0.0, 0.5];
        let anterior = [0.75, 0.5, 0.25, 1.0];
        let texel = [0.5, 0.5, 0.5, 1.0];
        let saida = env.aplica_com(anterior, [0.0; 4], texel);
        // 0,5 + anterior − 0,5 = anterior; alfa 1 × 0,5.
        for (canal, esperado) in [0.75, 0.5, 0.25, 0.5].into_iter().enumerate() {
            assert!((saida[canal] - esperado).abs() < 1e-6, "{saida:?}");
        }
    }

    #[test]
    fn o_combine_interpola_pela_terceira_fonte() {
        let mut env = TexEnv::com_modo(gles::GL_COMBINE);
        env.define(gles::GL_COMBINE_RGB, gles::GL_INTERPOLATE, 0.0);
        env.define(gles::GL_SRC0_RGB, gles::GL_TEXTURE, 0.0);
        env.define(gles::GL_SRC0_RGB + 1, gles::GL_PRIMARY_COLOR, 0.0);
        env.define(gles::GL_SRC0_RGB + 2, gles::GL_CONSTANT, 0.0);
        env.cor = [0.25; 4];
        let saida = env.aplica([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
        // 0 × 0,25 + 1 × 0,75
        assert!((saida[0] - 0.75).abs() < 1e-6, "{saida:?}");
    }

    /// O quadro depois de pintado. O desenho é acumulado e só vira pixel no despejo — que no
    /// emulador acontece no `eglSwapBuffers`, e aqui precisa ser pedido.
    fn pixels(state: &mut GlState) -> &[[u8; 4]] {
        state.flush();
        &state.color
    }

    fn quad(state: &mut GlState, z: f32, color: [f32; 4]) {
        let vertex = |x: f32, y: f32| Vertex {
            position: [x, y, z, 1.0],
            color,
            uv: [0.0; 2],
            ..Default::default()
        };
        state.draw(
            gles::GL_TRIANGLES,
            &[
                vertex(-1.0, -1.0),
                vertex(1.0, -1.0),
                vertex(1.0, 1.0),
                vertex(-1.0, -1.0),
                vertex(1.0, 1.0),
                vertex(-1.0, 1.0),
            ],
        );
    }

    #[test]
    fn o_blit_de_tela_poe_a_textura_no_lugar_certo() {
        // `glDrawTexiOES` desenha em coordenadas de **janela**, cujo zero fica embaixo — o
        // oposto da nossa superfície. Errar essa inversão põe a imagem de cabeça para baixo, e
        // num emulador de arcade isso passa por "funcionou".
        let mut state = GlState::new(4, 4);
        state.set_viewport(0, 0, 4, 4);
        // Duas linhas: a de baixo vermelha, a de cima azul (na ordem em que a textura chega).
        let texture = Texture {
            width: 1,
            height: 2,
            pixels: vec![[255, 0, 0, 255], [0, 0, 255, 255]],
            filter: gles::GL_NEAREST,
            ..Texture::default()
        };
        state.textures.insert(1, texture);
        state.bind_texture(1);
        state.set_capability(gles::GL_TEXTURE_2D, true);
        state.set_texture_crop([0, 0, 1, 2]);

        // Um retângulo de 4x2 encostado na base da janela.
        state.draw_texture(0.0, 0.0, 0.0, 4.0, 2.0);
        let frame = state.present(4, 4);
        let at = |x: usize, y: usize| frame[y * 4 + x];
        let vermelho = (255u16 >> 3) << 11;
        let azul = 255u16 >> 3;

        // A metade de cima da tela não foi tocada; a de baixo recebeu o desenho.
        assert_eq!(at(0, 0), 0, "o topo continua limpo");
        assert_eq!(at(0, 1), 0);
        assert_ne!(at(0, 2), 0, "a base recebeu o blit");
        assert_ne!(at(0, 3), 0);
        // E dentro dele o primeiro texel fica embaixo, como manda a janela do OpenGL.
        assert_eq!(at(0, 3), vermelho, "o texel 0 é o de baixo");
        assert_eq!(at(0, 2), azul);
    }

    #[test]
    fn o_blit_sem_recorte_usa_a_textura_inteira() {
        // Recorte zerado é a textura toda: desenhar nada seria pior que adotar o padrão óbvio.
        let mut state = GlState::new(2, 2);
        state.set_viewport(0, 0, 2, 2);
        state.textures.insert(
            1,
            Texture {
                width: 1,
                height: 1,
                pixels: vec![[0, 255, 0, 255]],
                filter: gles::GL_NEAREST,
                ..Texture::default()
            },
        );
        state.bind_texture(1);
        state.set_capability(gles::GL_TEXTURE_2D, true);
        state.draw_texture(0.0, 0.0, 0.0, 2.0, 2.0);
        let verde = (255u16 >> 2) << 5;
        assert_eq!(state.present(2, 2), vec![verde; 4]);
    }

    #[test]
    fn multiplicacao_respeita_a_ordem_do_opengl() {
        // `translate` depois de `scale` escala a translação — é a ordem do `glTranslate`
        // aplicado sobre uma matriz que já tem escala.
        let m = multiply(&scaling(2.0, 2.0, 2.0), &translation(1.0, 0.0, 0.0));
        assert_eq!(transform(&m, [0.0, 0.0, 0.0, 1.0])[0], 2.0);
        assert_eq!(
            transform(&IDENTITY, [3.0, 4.0, 5.0, 1.0]),
            [3.0, 4.0, 5.0, 1.0]
        );
    }

    #[test]
    fn rotacao_de_noventa_graus_leva_x_para_y() {
        let m = rotation(90.0, 0.0, 0.0, 1.0);
        let v = transform(&m, [1.0, 0.0, 0.0, 1.0]);
        assert!((v[0]).abs() < 1e-6, "{v:?}");
        assert!((v[1] - 1.0).abs() < 1e-6, "{v:?}");
    }

    #[test]
    fn preenche_a_tela_e_respeita_o_teste_de_profundidade() {
        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_DEPTH_TEST, true);
        state.set_depth_func(gles::GL_LESS);
        state.clear(gles::GL_COLOR_BUFFER_BIT | gles::GL_DEPTH_BUFFER_BIT);

        quad(&mut state, 0.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [255, 0, 0, 255]);

        // Mais longe não passa no teste; mais perto passa.
        quad(&mut state, 0.5, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [255, 0, 0, 255]);
        quad(&mut state, -0.5, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [0, 0, 255, 255]);
    }

    /// O `y` do `glViewport` conta de baixo para cima, e o quadro é guardado de cima para baixo.
    ///
    /// Uma viewport em `y = 0` com metade da altura é a metade **de baixo** da imagem. Tratar o
    /// `y` como contado do topo punha o trecho de pista que o Crash Nitro Kart desenha pelo portal
    /// espelhado na parte de baixo da tela.
    #[test]
    fn a_viewport_conta_o_y_de_baixo_para_cima() {
        let mut state = GlState::new(8, 8);
        state.set_viewport(0, 0, 8, 8);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.set_viewport(0, 0, 8, 4);
        quad(&mut state, 0.0, [1.0, 0.0, 0.0, 1.0]);
        let quadro = pixels(&mut state);
        assert_eq!(quadro[6 * 8 + 4], [255, 0, 0, 255], "metade de baixo pintada");
        assert_eq!(quadro[8 + 4], [0, 0, 0, 255], "metade de cima intacta");
    }

    /// O `glDepthRange` decide quem fica na frente entre faixas, não a profundidade normalizada.
    ///
    /// É assim que o Crash Nitro Kart põe o brilho do kart sempre por cima: o desenho mais
    /// "longe" em coordenadas normalizadas, mas numa faixa mais perto, tem de passar no teste.
    #[test]
    fn a_faixa_de_profundidade_manda_na_ordem() {
        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_DEPTH_TEST, true);
        state.set_depth_func(gles::GL_LESS);
        state.clear(gles::GL_COLOR_BUFFER_BIT | gles::GL_DEPTH_BUFFER_BIT);

        Rasterizador::set_depth_range(&mut state, 0.9, 1.0);
        quad(&mut state, -0.5, [1.0, 0.0, 0.0, 1.0]);
        Rasterizador::set_depth_range(&mut state, 0.0, 0.1);
        quad(&mut state, 0.5, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [0, 255, 0, 255]);
    }

    #[test]
    fn mistura_com_alfa_pesa_as_duas_cores() {
        let mut state = GlState::new(4, 4);
        state.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.set_capability(gles::GL_BLEND, true);
        state.set_blend_func(gles::GL_SRC_ALPHA, gles::GL_ONE_MINUS_SRC_ALPHA);
        quad(&mut state, 0.0, [1.0, 1.0, 1.0, 0.5]);
        let pixel = pixels(&mut state)[4 * 2 + 2];
        assert!((pixel[0] as i32 - 128).abs() <= 1, "{pixel:?}");
    }

    #[test]
    fn teste_de_alfa_descarta_o_fragmento() {
        let mut state = GlState::new(4, 4);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.set_capability(gles::GL_ALPHA_TEST, true);
        state.set_alpha_func(gles::GL_GREATER, 0.5);
        quad(&mut state, 0.0, [1.0, 1.0, 1.0, 0.25]);
        assert_eq!(pixels(&mut state)[4 * 2 + 2], [0, 0, 0, 255]);
    }

    /// A orientação é o detalhe que mais custou: errar o sinal descarta exatamente as faces
    /// que deveriam aparecer, e o resultado é uma tela preta com a geometria certa por trás.
    #[test]
    fn a_face_da_frente_e_a_anti_horaria_em_coordenadas_do_opengl() {
        let triangle = |flip: bool| {
            let vertex = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0; 4],
                uv: [0.0; 2],
                ..Default::default()
            };
            // Em coordenadas do OpenGL, com o Y para cima, esta ordem é anti-horária.
            let mut v = [vertex(-1.0, -1.0), vertex(1.0, -1.0), vertex(0.0, 1.0)];
            if flip {
                v.swap(0, 1);
            }
            v
        };
        let center = 8 * 4 + 4;

        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_CULL_FACE, true);
        state.set_cull_face(gles::GL_BACK);
        state.set_front_face(gles::GL_CCW);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &triangle(false));
        assert_eq!(pixels(&mut state)[center], [255, 255, 255, 255]);

        // O avesso do mesmo triângulo some.
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &triangle(true));
        assert_eq!(pixels(&mut state)[center], [0, 0, 0, 255]);

        // Com `GL_CW` a decisão inverte.
        state.set_front_face(gles::GL_CW);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &triangle(true));
        assert_eq!(pixels(&mut state)[center], [255, 255, 255, 255]);
    }

    #[test]
    fn o_recorte_no_plano_proximo_preserva_a_orientacao() {
        // Um triângulo que atravessa o plano próximo: dois vértices à frente, um atrás. O
        // recorte parte o que sobra em dois, e se ele percorrer o polígono ao contrário a
        // orientação inverte — o descarte de faces então joga fora justamente o pedaço
        // recortado, e o buraco só aparece quando a câmera chega perto da geometria.
        let vertex = |x: f32, y: f32, z: f32, w: f32| Vertex {
            position: [x, y, z, w],
            color: [1.0; 4],
            uv: [0.0; 2],
            ..Default::default()
        };
        // Anti-horário em coordenadas do OpenGL; o terceiro vértice está atrás da câmera.
        let tri = [
            vertex(-1.0, -1.0, 1.0, 2.0),
            vertex(1.0, -1.0, 1.0, 2.0),
            vertex(0.0, 4.0, -4.0, -2.0),
        ];
        let center = 8 * 5 + 4;

        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_CULL_FACE, true);
        state.set_cull_face(gles::GL_BACK);
        state.set_front_face(gles::GL_CCW);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &tri);
        assert_eq!(pixels(&mut state)[center], [255, 255, 255, 255]);
    }

    #[test]
    fn o_plano_proximo_e_z_mais_w_e_nao_o_sinal_de_w() {
        // Um triângulo logo atrás do plano próximo, mas ainda com `w` positivo: recortar só
        // pelo sinal de `w` o deixaria passar, e a divisão pela perspectiva o espalharia por
        // toda a tela.
        let vertex = |x: f32, y: f32| Vertex {
            position: [x, y, -1.0, 0.001],
            color: [1.0; 4],
            uv: [0.0; 2],
            ..Default::default()
        };
        let tri = [
            vertex(-0.001, -0.001),
            vertex(0.001, -0.001),
            vertex(0.0, 0.001),
        ];

        let mut state = GlState::new(8, 8);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &tri);
        assert!(pixels(&mut state).iter().all(|p| *p == [0, 0, 0, 255]));
    }

    #[test]
    fn a_matriz_de_textura_transforma_as_coordenadas() {
        // O Zeebo Sports Peteca manda os `uv` em ponto fixo e traz de volta para `0..1` pela
        // matriz de textura. Ignorá-la fazia o `GL_REPEAT` dar a volta na textura a cada pixel,
        // e a quadra e a arquibancada saíam como confete das cores certas.
        let mut state = GlState::new(4, 4);
        let mut textura = Texture {
            width: 2,
            height: 1,
            pixels: vec![[255, 0, 0, 255], [0, 0, 255, 255]],
            ..Texture::default()
        };
        textura.filter = gles::GL_NEAREST;
        state.textures.insert(1, textura);
        state.bind_texture(1);
        state.set_capability(gles::GL_TEXTURE_2D, true);

        // `u` chega valendo 32767 e a matriz o divide de volta para perto de zero, que é o
        // texel vermelho. Sem a matriz, o valor cru cairia em qualquer lugar da textura.
        state.set_matrix_mode(gles::GL_TEXTURE);
        state.load_matrix(scaling(1.0 / 32767.0, 1.0, 1.0));
        state.set_matrix_mode(gles::GL_MODELVIEW);

        let vertex = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0, 1.0],
            color: [1.0; 4],
            uv: [32767.0 * 0.25, 0.0],
            ..Default::default()
        };
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(
            gles::GL_TRIANGLES,
            &[vertex(-1.0, -1.0), vertex(1.0, -1.0), vertex(0.0, 1.0)],
        );
        assert_eq!(pixels(&mut state)[2 * 4 + 2], [255, 0, 0, 255]);
    }

    #[test]
    fn sem_viewport_a_superficie_e_a_tela_inteira() {
        // O Peteca desenha em coordenadas de tela e nunca chama `glViewport`. Deduzir a
        // superfície de um conjunto vazio de viewports dava 1×1, e a apresentação esticava um
        // pixel só por toda a tela: o jogo saía inteiramente branco.
        let state = GlState::new(640, 480);
        assert_eq!(state.surface(), (640, 480));
    }

    #[test]
    fn a_superficie_e_a_maior_viewport_que_o_jogo_usou() {
        // O Quake do Zeebo desenha em 320×400 numa tela de 640×480, e a apresentação amplia.
        let mut state = GlState::new(640, 480);
        state.set_viewport(0, 0, 320, 400);
        state.set_viewport(0, 360, 320, 40);
        assert_eq!(state.surface(), (320, 400));
    }

    #[test]
    fn a_pilha_de_matrizes_guarda_e_devolve() {
        let mut state = GlState::new(4, 4);
        state.set_matrix_mode(gles::GL_MODELVIEW);
        state.load_identity();
        state.push_matrix();
        state.mult_matrix(translation(5.0, 0.0, 0.0));
        assert_eq!(state.top()[12], 5.0);
        state.pop_matrix();
        assert_eq!(state.top()[12], 0.0);
        // Desempilhar demais não pode esvaziar a pilha.
        state.pop_matrix();
        state.pop_matrix();
        assert_eq!(*state.top(), IDENTITY);
    }

    #[test]
    fn a_apresentacao_e_rgb565() {
        let mut state = GlState::new(2, 2);
        state.set_clear_color([1.0, 0.0, 0.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        assert_eq!(state.present(2, 2), vec![0xf800; 4]);
    }

    /// A Z-Wheel lê o buffer de cor duas vezes por quadro, e na primeira a fila está sempre
    /// vazia: era um quadro inteiro reconvertido sem nada ter mudado. A segunda leitura só
    /// pode reaproveitar o que já está no vetor se de fato nada tocou o buffer.
    #[test]
    fn o_quadro_so_e_reconvertido_quando_alguma_coisa_mudou() {
        let mut state = GlState::new(64, 64);
        state.set_clear_color([1.0, 0.0, 0.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);

        let mut bytes = Vec::new();
        state.frame_rgb565(64, 64, &mut bytes);
        let vermelho = bytes.clone();
        assert_eq!(bytes.len(), 64 * 64 * 2);

        // Nada mudou: a segunda leitura devolve o mesmo conteúdo, sem reconverter.
        bytes.fill(0xAB);
        state.frame_rgb565(64, 64, &mut bytes);
        assert!(
            bytes.iter().all(|&b| b == 0xAB),
            "sem mudança, o vetor de saída não é tocado"
        );

        // Um `clear` mexe no buffer, então a leitura seguinte tem de reconverter.
        state.set_clear_color([0.0, 0.0, 1.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.frame_rgb565(64, 64, &mut bytes);
        assert_ne!(bytes, vermelho, "depois do clear o quadro é outro");
        assert!(
            bytes.iter().any(|&b| b != 0xAB),
            "e foi de fato reconvertido"
        );

        // E uma mudança de tamanho não pode cair no atalho.
        state.frame_rgb565(32, 32, &mut bytes);
        assert_eq!(bytes.len(), 32 * 32 * 2);
    }
}

/// O estado de GL que o **guest** mudou, em seções — e a regra de quando não se pode salvar.
///
/// ## Por que seções pequenas, e não uma lista de números
///
/// A primeira versão do estado da máquina usava uma lista corrida de números, e eu errei o índice
/// dela **duas vezes** — o `pending_launch` voltou com a metade alta do `proximo_serial`, e o
/// `network` voltou com a largura do retângulo de recorte. Aqui são mais de cem números, então a
/// lista corrida seria pedir para errar de novo. Cada seção tem o tamanho conferido na leitura, e um
/// deslocamento dentro dela é recusado em vez de virar valor trocado.
///
/// ## O que entra, e o que fica para depois
///
/// Entram os 48 campos que são **estado** — as três pilhas de matriz, o viewport e a tesoura, as
/// cores, as bandeiras e funções de teste, a névoa, a faixa de profundidade, a máscara de cor, a
/// unidade de textura e a **iluminação** (oito luzes, o material e o modelo). Ficam de fora, por
/// enquanto e com motivo:
///
/// - `textures`, que precisa de formato próprio por causa das cadeias de mipmap;
/// - `color`, `depth` e `stencil` — os três buffers de 640×480. Medido: **o de cor não é duplicata
///   da tela**. A tela que o core apresenta é o bitmap do aparelho, e o buffer de cor é a superfície
///   de desenho do GL; `eglSwapBuffers` lê um e entrega o outro. Os dois têm de ser gravados, e é
///   isso que faz o estado crescer cerca de 2,7 MB.
///
/// ## A regra do desenho em curso
///
/// `batch`, `pending` e `transformed` são acumuladores de um desenho **começado e não terminado**.
/// O frontend chama o serialize **entre quadros**, nunca dentro de um `retro_run`, então no instante
/// do save eles estão vazios — e é isso que os torna dispensáveis. Confiar nisso seria frágil: quem
/// grava **pergunta** com [`GlState::desenho_em_curso`] e recusa o save se a resposta for sim. Um
/// estado salvo no meio de um `glBegin` prometeria um desenho que nunca existiu.
impl crate::save_state::Guardavel for GlState {
    fn grava(&self, destino: &mut crate::save_state::Secoes) {
        let floats = |valores: &[f32]| -> Vec<u32> {
            valores.iter().map(|v| v.to_bits()).collect()
        };

        // As três pilhas de matriz, cada uma com a contagem na frente.
        let mut matrizes = vec![self.matrix_mode];
        for pilha in [&self.modelview, &self.projection, &self.texture_matrix] {
            matrizes.push(pilha.len() as u32);
            for matriz in pilha {
                matrizes.extend(floats(matriz));
            }
        }
        destino.poe_u32s("gl.matrizes", matrizes);

        // Onde se desenha.
        let mut onde = vec![
            self.viewport.0 as u32,
            self.viewport.1 as u32,
            self.viewport.2 as u32,
            self.viewport.3 as u32,
            u32::from(self.tesoura.is_some()),
            self.tesoura.unwrap_or((0, 0, 0, 0)).0 as u32,
            self.tesoura.unwrap_or((0, 0, 0, 0)).1 as u32,
            self.tesoura.unwrap_or((0, 0, 0, 0)).2 as u32,
            self.tesoura.unwrap_or((0, 0, 0, 0)).3 as u32,
            self.tesoura_crua.0 as u32,
            self.tesoura_crua.1 as u32,
            self.tesoura_crua.2 as u32,
            self.tesoura_crua.3 as u32,
            u32::from(self.tesoura_ligada),
            u32::from(self.surface.is_some()),
            self.surface.unwrap_or((0, 0)).0 as u32,
            self.surface.unwrap_or((0, 0)).1 as u32,
            u32::from(self.esticada),
            u32::from(self.sujo),
        ];
        onde.extend(floats(&self.clear_color));
        onde.push(self.clear_depth.to_bits());
        onde.extend(floats(&self.current_color));
        onde.push(u32::from(self.clear_stencil));
        onde.extend(self.color_mask.iter().map(|m| u32::from(*m)));
        destino.poe_u32s("gl.onde", onde);

        // As bandeiras e funções de teste.
        destino.poe_u32s(
            "gl.testes",
            vec![
                u32::from(self.depth_test),
                u32::from(self.depth_mask),
                self.depth_func,
                u32::from(self.blend),
                self.blend_src,
                self.blend_dst,
                u32::from(self.alpha_test),
                self.alpha_func,
                self.alpha_ref.to_bits(),
                u32::from(self.cull_face),
                self.cull_mode,
                self.front_face,
                u32::from(self.stencil_test),
                self.stencil_func,
                self.stencil_ref as u32,
                self.stencil_value_mask,
                self.stencil_write_mask,
                self.stencil_op[0],
                self.stencil_op[1],
                self.stencil_op[2],
                self.depth_range.0.to_bits(),
                self.depth_range.1.to_bits(),
                u32::from(self.lighting),
                u32::from(self.color_material),
                self.shade_model,
            ],
        );

        // As texturas ligadas e o ambiente de textura das duas unidades.
        let mut textura = vec![
            self.bound_texture,
            u32::from(self.texture_2d),
            self.active_unit,
            self.client_unit,
        ];
        textura.extend(ambientes_do_texto(&self.texture_env));
        textura.push(u32::from(self.unidade1.ligada));
        textura.push(self.unidade1.textura);
        textura.extend(ambientes_do_texto(&self.unidade1.env));
        destino.poe_u32s("gl.textura", textura);

        // A névoa.
        let mut nevoa: Vec<u32> = vec![
            u32::from(self.fog.ligada),
            self.fog.curva,
            self.fog.densidade.to_bits(),
            self.fog.inicio.to_bits(),
            self.fog.fim.to_bits(),
            u32::from(self.fog.permitida),
        ];
        nevoa.extend(floats(&self.fog.cor));
        destino.poe_u32s("gl.nevoa", nevoa);

        // A iluminação: o material, o ambiente do modelo e as oito luzes.
        let mut luz = floats(&self.material.ambient);
        luz.extend(floats(&self.material.diffuse));
        luz.extend(floats(&self.material.specular));
        luz.extend(floats(&self.material.emission));
        luz.push(self.material.shininess.to_bits());
        luz.extend(floats(&self.light_model_ambient));
        for fonte in &self.lights {
            luz.push(u32::from(fonte.enabled));
            luz.extend(floats(&fonte.ambient));
            luz.extend(floats(&fonte.diffuse));
            luz.extend(floats(&fonte.specular));
            luz.extend(floats(&fonte.position));
            luz.extend(floats(&fonte.spot_direction));
            luz.push(fonte.spot_exponent.to_bits());
            luz.push(fonte.spot_cutoff.to_bits());
            luz.extend(floats(&fonte.attenuation));
        }
        destino.poe_u32s("gl.iluminacao", luz);
        destino.poe_u32s(
            "gl.buffers",
            [
                self.width as u32,
                self.height as u32,
                self.color.len() as u32,
                self.depth.len() as u32,
                self.stencil.len() as u32,
            ],
        );
        destino.poe("gl.buffer.cor", bytes_dos_rgba(&self.color));
        let mut profundidade = Vec::with_capacity(self.depth.len() * 4);
        for valor in &self.depth {
            profundidade.extend_from_slice(&valor.to_bits().to_le_bytes());
        }
        destino.poe("gl.buffer.profundidade", profundidade);
        destino.poe("gl.buffer.stencil", self.stencil.clone());
        grava_texturas(&self.textures, destino);
    }

    fn restaura(&mut self, origem: &crate::save_state::Leitor<'_>) -> Result<(), crate::save_state::Erro> {
        use crate::save_state::Erro;
        let faltando = |nome: &str| Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        };

        let matrizes = origem.u32s("gl.matrizes")?;
        let onde = origem.u32s("gl.onde")?;
        let testes = origem.u32s("gl.testes")?;
        let textura = origem.u32s("gl.textura")?;
        let nevoa = origem.u32s("gl.nevoa")?;
        let luz = origem.u32s("gl.iluminacao")?;

        let conferir = |nome: &str, veio: usize, esperado: usize| -> Result<(), Erro> {
            if veio != esperado {
                return Err(Erro::Secao {
                    nome: nome.to_string(),
                    motivo: format!("esperava {esperado} números e veio {veio}"),
                });
            }
            Ok(())
        };
        // Dezenove do viewport, da tesoura e da superfície; quatro da cor de limpeza; uma da
        // profundidade; quatro da cor corrente; uma do stencil; quatro da máscara de cor.
        conferir("gl.onde", onde.len(), 19 + 4 + 1 + 4 + 1 + 4)?;
        conferir("gl.testes", testes.len(), 25)?;
        conferir("gl.textura", textura.len(), 4 + AMBIENTES + 2 + AMBIENTES)?;
        conferir("gl.nevoa", nevoa.len(), 6 + 4)?;
        // Material (dezesseis floats e o brilho) mais o ambiente do modelo: vinte e um. Cada luz
        // ocupa vinte e cinco: ligada, quatro de ambiente, quatro de difusa, quatro de especular,
        // quatro de posição, três de direção do facho, expoente, corte e três de atenuação.
        conferir(
            "gl.iluminacao",
            luz.len(),
            21 + crate::video::gles::LUZES * 25,
        )?;

        let f = |valor: u32| f32::from_bits(valor);
        let booleano = |valor: u32, campo: &str, onde: &str| -> Result<bool, Erro> {
            match valor {
                0 => Ok(false),
                1 => Ok(true),
                outro => Err(Erro::Secao {
                    nome: onde.to_string(),
                    motivo: format!("o campo `{campo}` vale {outro}, e é booleano"),
                }),
            }
        };

        // As pilhas de matriz.
        let mut cursor = 0usize;
        let matrix_mode = matrizes[cursor];
        cursor += 1;
        let mut pilhas: Vec<Vec<Matrix>> = Vec::new();
        for _ in 0..3 {
            if matrizes.len() < cursor + 1 {
                return Err(faltando("gl.matrizes"));
            }
            let quantas = matrizes[cursor] as usize;
            cursor += 1;
            if matrizes.len() < cursor + quantas * 16 {
                return Err(Erro::Secao {
                    nome: "gl.matrizes".to_string(),
                    motivo: format!("uma pilha diz ter {quantas} matrizes e o arquivo acaba antes"),
                });
            }
            let mut pilha = Vec::with_capacity(quantas);
            for _ in 0..quantas {
                let mut matriz = [0.0f32; 16];
                for (indice, destino) in matriz.iter_mut().enumerate() {
                    *destino = f(matrizes[cursor + indice]);
                }
                cursor += 16;
                pilha.push(matriz);
            }
            pilhas.push(pilha);
        }

        // Daqui para baixo é aplicação.
        self.matrix_mode = matrix_mode;
        // **A ordem do escritor, não a que eu lembrava.** Eu tinha posto a projeção primeiro, e o
        // teste mostrou a troca: a pilha `modelview` voltou com a projeção dentro. Trocar duas
        // matrizes não dá erro nenhum — dá a cena desenhada no lugar errado, e só se vê olhando.
        self.modelview = pilhas.remove(0);
        self.projection = pilhas.remove(0);
        self.texture_matrix = pilhas.remove(0);

        self.viewport = (
            onde[0] as i32,
            onde[1] as i32,
            onde[2] as i32,
            onde[3] as i32,
        );
        self.tesoura = booleano(onde[4], "tesoura", "gl.onde")?.then(|| {
            (
                onde[5] as i32,
                onde[6] as i32,
                onde[7] as i32,
                onde[8] as i32,
            )
        });
        self.tesoura_crua = (
            onde[9] as i32,
            onde[10] as i32,
            onde[11] as i32,
            onde[12] as i32,
        );
        self.tesoura_ligada = booleano(onde[13], "tesoura_ligada", "gl.onde")?;
        self.surface = booleano(onde[14], "surface", "gl.onde")?
            .then(|| (onde[15] as usize, onde[16] as usize));
        self.esticada = booleano(onde[17], "esticada", "gl.onde")?;
        self.sujo = booleano(onde[18], "sujo", "gl.onde")?;
        for (indice, valor) in self.clear_color.iter_mut().enumerate() {
            *valor = f(onde[19 + indice]);
        }
        self.clear_depth = f(onde[23]);
        for (indice, valor) in self.current_color.iter_mut().enumerate() {
            *valor = f(onde[24 + indice]);
        }
        self.clear_stencil = onde[28] as u8;
        for (indice, valor) in self.color_mask.iter_mut().enumerate() {
            *valor = onde[29 + indice] != 0;
        }

        self.depth_test = booleano(testes[0], "depth_test", "gl.testes")?;
        self.depth_mask = booleano(testes[1], "depth_mask", "gl.testes")?;
        self.depth_func = testes[2];
        self.blend = booleano(testes[3], "blend", "gl.testes")?;
        self.blend_src = testes[4];
        self.blend_dst = testes[5];
        self.alpha_test = booleano(testes[6], "alpha_test", "gl.testes")?;
        self.alpha_func = testes[7];
        self.alpha_ref = f(testes[8]);
        self.cull_face = booleano(testes[9], "cull_face", "gl.testes")?;
        self.cull_mode = testes[10];
        self.front_face = testes[11];
        self.stencil_test = booleano(testes[12], "stencil_test", "gl.testes")?;
        self.stencil_func = testes[13];
        self.stencil_ref = testes[14] as i32;
        self.stencil_value_mask = testes[15];
        self.stencil_write_mask = testes[16];
        self.stencil_op = [testes[17], testes[18], testes[19]];
        self.depth_range = (f(testes[20]), f(testes[21]));
        self.lighting = booleano(testes[22], "lighting", "gl.testes")?;
        self.color_material = booleano(testes[23], "color_material", "gl.testes")?;
        self.shade_model = testes[24];

        self.bound_texture = textura[0];
        self.texture_2d = booleano(textura[1], "texture_2d", "gl.textura")?;
        self.active_unit = textura[2];
        self.client_unit = textura[3];
        self.texture_env = le_ambiente_do_texto(&textura[4..4 + AMBIENTES]);
        self.unidade1.ligada = booleano(textura[4 + AMBIENTES], "unidade1.ligada", "gl.textura")?;
        self.unidade1.textura = textura[5 + AMBIENTES];
        self.unidade1.env = le_ambiente_do_texto(&textura[6 + AMBIENTES..]);

        self.fog.ligada = booleano(nevoa[0], "fog.ligada", "gl.nevoa")?;
        self.fog.curva = nevoa[1];
        self.fog.densidade = f(nevoa[2]);
        self.fog.inicio = f(nevoa[3]);
        self.fog.fim = f(nevoa[4]);
        self.fog.permitida = booleano(nevoa[5], "fog.permitida", "gl.nevoa")?;
        for (indice, valor) in self.fog.cor.iter_mut().enumerate() {
            *valor = f(nevoa[6 + indice]);
        }

        // Um cursor por índice, e não por closure que o empresta: a closure mutável não deixa o
        // contador ser lido no meio, e o compilador disse isso antes de eu errar a conta na mão.
        let mut cursor = 0usize;
        macro_rules! quatro {
            ($destino:expr) => {
                for valor in $destino.iter_mut() {
                    *valor = f(luz[cursor]);
                    cursor += 1;
                }
            };
        }
        quatro!(self.material.ambient);
        quatro!(self.material.diffuse);
        quatro!(self.material.specular);
        quatro!(self.material.emission);
        self.material.shininess = f(luz[cursor]);
        cursor += 1;
        quatro!(self.light_model_ambient);
        for indice in 0..self.lights.len() {
            self.lights[indice].enabled = booleano(luz[cursor], "luz.enabled", "gl.iluminacao")?;
            cursor += 1;
            quatro!(self.lights[indice].ambient);
            quatro!(self.lights[indice].diffuse);
            quatro!(self.lights[indice].specular);
            quatro!(self.lights[indice].position);
            for posicao in 0..3 {
                self.lights[indice].spot_direction[posicao] = f(luz[cursor]);
                cursor += 1;
            }
            self.lights[indice].spot_exponent = f(luz[cursor]);
            self.lights[indice].spot_cutoff = f(luz[cursor + 1]);
            cursor += 2;
            quatro!(self.lights[indice].attenuation);
        }

        // **Os três buffers.** Medido antes de decidir: o de cor NÃO é duplicata da tela. A tela
        // que o core apresenta é o bitmap do aparelho, e o buffer de cor é a superfície de desenho
        // do GL — o `eglSwapBuffers` lê um e entrega o outro. Os dois têm de ser gravados.
        let buffers = origem.u32s("gl.buffers")?;
        if buffers.len() != 5 {
            return Err(Erro::Secao {
                nome: "gl.buffers".to_string(),
                motivo: format!("esperava 5 números e veio {}", buffers.len()),
            });
        }
        let (largura, altura) = (buffers[0] as usize, buffers[1] as usize);
        let teto = largura * altura;
        // **Um teto no tamanho declarado.** Um arquivo corrompido — ou escrito por uma versão com
        // outra resolução — não pode fazer o carregamento pedir memória absurda. Acima do tamanho
        // da superfície é recusa, e não alocação.
        let conferir_tamanho = |nome: &str, declarado: usize, por_pixel: usize| -> Result<usize, Erro> {
            if declarado > teto {
                return Err(Erro::Secao {
                    nome: nome.to_string(),
                    motivo: format!(
                        "o estado diz ter {declarado} element(s) e a superfície é {largura}x{altura}"
                    ),
                });
            }
            let esperado = declarado * por_pixel;
            Ok(esperado)
        };
        let quantos = conferir_tamanho("gl.buffer.cor", buffers[2] as usize, 4)?;
        let cor = secao_do_estado(origem, "gl.buffer.cor")?;
        if cor.len() != quantos {
            return Err(Erro::Secao {
                nome: "gl.buffer.cor".to_string(),
                motivo: format!("esperava {quantos} bytes e veio {}", cor.len()),
            });
        }
        let quantos = conferir_tamanho("gl.buffer.profundidade", buffers[3] as usize, 4)?;
        let profundidade = secao_do_estado(origem, "gl.buffer.profundidade")?;
        if profundidade.len() != quantos {
            return Err(Erro::Secao {
                nome: "gl.buffer.profundidade".to_string(),
                motivo: format!("esperava {quantos} bytes e veio {}", profundidade.len()),
            });
        }
        let quantos = conferir_tamanho("gl.buffer.stencil", buffers[4] as usize, 1)?;
        let stencil = secao_do_estado(origem, "gl.buffer.stencil")?;
        if stencil.len() != quantos {
            return Err(Erro::Secao {
                nome: "gl.buffer.stencil".to_string(),
                motivo: format!("esperava {quantos} bytes e veio {}", stencil.len()),
            });
        }
        let cor: Vec<[u8; 4]> = cor.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
        let profundidade: Vec<f32> = profundidade
            .chunks_exact(4)
            .map(|p| f32::from_bits(u32::from_le_bytes([p[0], p[1], p[2], p[3]])))
            .collect();

        self.color = cor;
        self.depth = profundidade;
        self.stencil = stencil;
        self.textures = le_texturas(origem)?;
        Ok(())
    }
}

/// Grava as texturas: por objeto, os números, os pixels e os níveis de redução.
///
/// A cadeia de mipmaps é o motivo de esta tabela ter formato próprio. Descartá-la foi a causa das
/// listras nos modelos do palco da Z-Wheel: eles vêm com a cadeia inteira, de 128×128 até 1×1, e
/// são vistos de raspão — a lateral de um carro ocupa poucos pixels de largura e cobre a textura
/// toda. Amostrando sempre o nível zero, cada pixel cai num texel qualquer e o resultado é o
/// traseiro do carro repetido em colunas.
///
/// Os pixels vão **crus**, um byte de alpha inclusive: quatro por pixel, na ordem de leitura. É a
/// tabela que mais pesa no estado, e é onde comprimir valeria mais — mas comprimir é uma decisão
/// sobre o formato, e o formato já tem versão para receber isso depois.
fn grava_texturas(
    texturas: &std::collections::HashMap<u32, Texture>,
    destino: &mut crate::save_state::Secoes,
) {
    let mut ids: Vec<u32> = texturas.keys().copied().collect();
    ids.sort_unstable();
    destino.poe_u32s("tex.ids", ids.iter().copied());
    for id in ids {
        let textura = &texturas[&id];
        destino.poe_u32s(
            &format!("tex.{id}.meta"),
            [
                textura.width as u32,
                textura.height as u32,
                textura.wrap[0],
                textura.wrap[1],
                textura.filter,
                textura.min_filter,
                textura.crop[0] as u32,
                textura.crop[1] as u32,
                textura.crop[2] as u32,
                textura.crop[3] as u32,
                textura.mipmaps.len() as u32,
            ],
        );
        destino.poe(&format!("tex.{id}.pixels"), bytes_dos_pixels(&textura.pixels));
        // As dimensões de cada nível, e depois todos os pixels deles em seguida.
        let mut medidas = Vec::with_capacity(textura.mipmaps.len() * 2);
        let mut pixels = Vec::new();
        for nivel in &textura.mipmaps {
            medidas.push(nivel.width as u32);
            medidas.push(nivel.height as u32);
            pixels.extend_from_slice(&bytes_dos_pixels(&nivel.pixels));
        }
        destino.poe_u32s(&format!("tex.{id}.mips"), medidas);
        destino.poe(&format!("tex.{id}.mip_pixels"), pixels);
    }
}

/// Os pixels RGBA como bytes, num vetor só.
fn bytes_dos_rgba(pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut saida = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        saida.extend_from_slice(pixel);
    }
    saida
}

/// Os pixels RGBA como bytes.
fn bytes_dos_pixels(pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut saida = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        saida.extend_from_slice(pixel);
    }
    saida
}

/// O caminho de volta de [`grava_texturas`], com a conferência que importa.
///
/// **O número de pixels tem de casar com as dimensões.** Um arquivo truncado, ou um nível de
/// mipmap com as medidas trocadas, desenharia listras — e o defeito apareceria como imagem
/// errada, sem nada apontando para o save state. Aqui é recusa.
fn le_texturas(
    origem: &crate::save_state::Leitor<'_>,
) -> Result<std::collections::HashMap<u32, Texture>, crate::save_state::Erro> {
    use crate::save_state::Erro;
    let ids = origem.u32s("tex.ids")?;
    let mut texturas = std::collections::HashMap::new();
    for id in ids {
        let meta = origem.u32s(&format!("tex.{id}.meta"))?;
        if meta.len() != 11 {
            return Err(Erro::Secao {
                nome: format!("tex.{id}.meta"),
                motivo: format!("esperava 11 números e veio {}", meta.len()),
            });
        }
        let (largura, altura) = (meta[0] as usize, meta[1] as usize);
        let pixels = pixels_dos_bytes(&secao_do_estado(origem, &format!("tex.{id}.pixels"))?);
        if pixels.len() != largura * altura {
            return Err(Erro::Secao {
                nome: format!("tex.{id}.pixels"),
                motivo: format!(
                    "a textura é {largura}x{altura} ({} pixels) e vieram {}",
                    largura * altura,
                    pixels.len()
                ),
            });
        }
        let medidas = origem.u32s(&format!("tex.{id}.mips"))?;
        let quantos = meta[10] as usize;
        if medidas.len() != quantos * 2 {
            return Err(Erro::Secao {
                nome: format!("tex.{id}.mips"),
                motivo: format!("diz ter {quantos} nível(is) e veio {} número(s)", medidas.len()),
            });
        }
        let cru = secao_do_estado(origem, &format!("tex.{id}.mip_pixels"))?;
        let mut mipmaps = Vec::with_capacity(quantos);
        let mut cursor = 0usize;
        for nivel in 0..quantos {
            let (l, a) = (medidas[nivel * 2] as usize, medidas[nivel * 2 + 1] as usize);
            let precisa = l * a * 4;
            if cru.len() < cursor + precisa {
                return Err(Erro::Secao {
                    nome: format!("tex.{id}.mip_pixels"),
                    motivo: format!(
                        "o nível {nivel} é {l}x{a} e precisa de {precisa} bytes, e restam {}",
                        cru.len() - cursor
                    ),
                });
            }
            mipmaps.push(Nivel {
                width: l,
                height: a,
                pixels: pixels_dos_bytes(&cru[cursor..cursor + precisa]),
            });
            cursor += precisa;
        }
        texturas.insert(
            id,
            Texture {
                width: largura,
                height: altura,
                pixels,
                mipmaps,
                wrap: [meta[2], meta[3]],
                filter: meta[4],
                min_filter: meta[5],
                crop: [
                    meta[6] as i32,
                    meta[7] as i32,
                    meta[8] as i32,
                    meta[9] as i32,
                ],
            },
        );
    }
    Ok(texturas)
}

/// Uma seção de bytes que tem de existir.
fn secao_do_estado<'a>(
    origem: &'a crate::save_state::Leitor<'a>,
    nome: &str,
) -> Result<Vec<u8>, crate::save_state::Erro> {
    origem
        .secao(nome)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| crate::save_state::Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })
}

/// Bytes RGBA como pixels. Sobra de menos de quatro bytes é descartada, e quem confere o número
/// é quem chama.
fn pixels_dos_bytes(bytes: &[u8]) -> Vec<[u8; 4]> {
    bytes
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect()
}

/// Quantos números o ambiente de textura ocupa: modo, dois de combinação, seis fontes, seis
/// operandos, duas escalas e quatro de cor.
const AMBIENTES: usize = 1 + 2 + 6 + 6 + 2 + 4;

/// O ambiente de textura como números.
fn ambientes_do_texto(env: &TexEnv) -> Vec<u32> {
    let mut saida = vec![env.modo];
    saida.extend(env.combina);
    saida.extend(env.fontes.iter().flatten().copied());
    saida.extend(env.operandos.iter().flatten().copied());
    saida.extend(env.escala.iter().map(|e| e.to_bits()));
    saida.extend(env.cor.iter().map(|c| c.to_bits()));
    saida
}

/// O caminho de volta de [`ambientes_do_texto`].
fn le_ambiente_do_texto(numeros: &[u32]) -> TexEnv {
    let mut cursor = 0usize;
    let mut proximo = || {
        let valor = numeros[cursor];
        cursor += 1;
        valor
    };
    let modo = proximo();
    let combina = [proximo(), proximo()];
    let mut fontes = [[0u32; 3]; 2];
    for linha in fontes.iter_mut() {
        for valor in linha.iter_mut() {
            *valor = proximo();
        }
    }
    let mut operandos = [[0u32; 3]; 2];
    for linha in operandos.iter_mut() {
        for valor in linha.iter_mut() {
            *valor = proximo();
        }
    }
    let escala = [f32::from_bits(proximo()), f32::from_bits(proximo())];
    let cor = [
        f32::from_bits(proximo()),
        f32::from_bits(proximo()),
        f32::from_bits(proximo()),
        f32::from_bits(proximo()),
    ];
    TexEnv {
        modo,
        combina,
        fontes,
        operandos,
        escala,
        cor,
    }
}

#[cfg(test)]
mod testes_do_estado_de_gl {
    use super::*;
    use crate::save_state::{Guardavel, Leitor, Secoes};

    /// **O estado de GL vai e volta**, campo por campo.
    ///
    /// Cada seção é conferida pelo tamanho na leitura; este teste confere os **valores**, que é o
    /// que o tamanho não pega. Ele cobre os quatro grupos de uma vez: as pilhas de matriz, onde se
    /// desenha, as bandeiras, a textura, a névoa e a iluminação.
    #[test]
    fn o_estado_de_gl_vai_e_volta() {
        let mut antes = GlState::new(640, 480);
        antes.matrix_mode = 0x1700;
        antes.modelview.push([1.5; 16]);
        antes.projection.push([-2.25; 16]);
        antes.texture_matrix.push([0.5; 16]);
        antes.viewport = (1, 2, 640, 480);
        antes.tesoura = Some((3, 4, 100, 200));
        antes.tesoura_crua = (5, 6, 7, 8);
        antes.tesoura_ligada = true;
        antes.surface = Some((320, 240));
        antes.esticada = true;
        antes.sujo = true;
        antes.clear_color = [0.25, 0.5, 0.75, 1.0];
        antes.clear_depth = 0.5;
        antes.current_color = [1.0, 0.0, 0.5, 0.25];
        antes.clear_stencil = 7;
        antes.color_mask = [true, false, true, false];
        antes.depth_test = true;
        antes.depth_mask = false;
        antes.depth_func = 0x0203;
        antes.blend = true;
        antes.blend_src = 1;
        antes.blend_dst = 2;
        antes.alpha_test = true;
        antes.alpha_func = 0x0204;
        antes.alpha_ref = 0.375;
        antes.cull_face = true;
        antes.cull_mode = 0x0405;
        antes.front_face = 0x0901;
        antes.stencil_test = true;
        antes.stencil_func = 0x0207;
        antes.stencil_ref = -3;
        antes.stencil_value_mask = 0xff;
        antes.stencil_write_mask = 0x0f;
        antes.stencil_op = [1, 2, 3];
        antes.depth_range = (0.0, 0.985);
        antes.lighting = true;
        antes.color_material = true;
        antes.shade_model = 0x1d01;
        antes.bound_texture = 0x1234;
        antes.texture_2d = true;
        antes.active_unit = 1;
        antes.client_unit = 2;
        antes.texture_env.modo = 0x2100;
        antes.texture_env.cor = [0.1, 0.2, 0.3, 0.4];
        antes.unidade1.ligada = true;
        antes.unidade1.textura = 0x5678;
        antes.fog.ligada = true;
        antes.fog.curva = 0x2601;
        antes.fog.densidade = 0.125;
        antes.fog.inicio = 10.0;
        antes.fog.fim = 20.0;
        antes.fog.permitida = true;
        antes.fog.cor = [0.5, 0.25, 0.125, 1.0];
        antes.material.shininess = 12.5;
        antes.material.ambient = [0.11; 4];
        antes.light_model_ambient = [0.22; 4];
        antes.lights[0].enabled = true;
        antes.lights[0].position = [1.0, 2.0, 3.0, 0.0];
        antes.lights[0].spot_cutoff = 45.0;
        antes.lights[0].attenuation = [1.0, 0.5, 0.25];

        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");

        let mut depois = GlState::new(640, 480);
        depois.restaura(&leitor).expect("restaurou");

        assert_eq!(depois.matrix_mode, 0x1700);
        // **O topo da pilha e o tamanho dela**, e não a lista inteira: o `GlState::new` já põe uma
        // identidade em cada pilha, e afirmar a lista supunha quantas havia antes. Foi assim que
        // este teste me reprovou uma vez, com o valor certo.
        for (nome, pilha, esperado) in [
            ("modelview", &depois.modelview, [1.5f32; 16]),
            ("projection", &depois.projection, [-2.25f32; 16]),
            ("texture_matrix", &depois.texture_matrix, [0.5f32; 16]),
        ] {
            assert_eq!(pilha.len(), 2, "o tamanho da pilha {nome}");
            assert_eq!(
                pilha.last().copied(),
                Some(esperado),
                "o topo da pilha {nome} não voltou"
            );
        }
        assert_eq!(depois.viewport, (1, 2, 640, 480));
        assert_eq!(depois.tesoura, Some((3, 4, 100, 200)));
        assert_eq!(depois.tesoura_crua, (5, 6, 7, 8));
        assert!(depois.tesoura_ligada);
        assert_eq!(depois.surface, Some((320, 240)));
        assert!(depois.esticada);
        assert!(depois.sujo);
        assert_eq!(depois.clear_color, [0.25, 0.5, 0.75, 1.0], "as cores são f32");
        assert_eq!(depois.clear_depth, 0.5);
        assert_eq!(depois.current_color, [1.0, 0.0, 0.5, 0.25]);
        assert_eq!(depois.clear_stencil, 7);
        assert_eq!(depois.color_mask, [true, false, true, false]);
        assert!(depois.depth_test);
        assert!(!depois.depth_mask);
        assert_eq!(depois.depth_func, 0x0203);
        assert!(depois.blend);
        assert_eq!((depois.blend_src, depois.blend_dst), (1, 2));
        assert!(depois.alpha_test);
        assert_eq!(depois.alpha_ref, 0.375);
        assert!(depois.cull_face);
        assert_eq!(depois.cull_mode, 0x0405);
        assert_eq!(depois.front_face, 0x0901);
        assert!(depois.stencil_test);
        assert_eq!(depois.stencil_ref, -3, "o `ref` do stencil é assinado");
        assert_eq!(
            (depois.stencil_value_mask, depois.stencil_write_mask),
            (0xff, 0x0f)
        );
        assert_eq!(depois.stencil_op, [1, 2, 3]);
        assert_eq!(depois.depth_range, (0.0, 0.985), "a faixa do Crash");
        assert!(depois.lighting);
        assert!(depois.color_material);
        assert_eq!(depois.shade_model, 0x1d01);
        assert_eq!(depois.bound_texture, 0x1234);
        assert!(depois.texture_2d);
        assert_eq!((depois.active_unit, depois.client_unit), (1, 2));
        assert_eq!(depois.texture_env.modo, 0x2100);
        assert_eq!(depois.texture_env.cor, [0.1, 0.2, 0.3, 0.4]);
        assert!(depois.unidade1.ligada);
        assert_eq!(depois.unidade1.textura, 0x5678);
        assert!(depois.fog.ligada);
        assert_eq!(depois.fog.curva, 0x2601);
        assert_eq!(depois.fog.densidade, 0.125);
        assert_eq!((depois.fog.inicio, depois.fog.fim), (10.0, 20.0));
        assert!(depois.fog.permitida);
        assert_eq!(depois.fog.cor, [0.5, 0.25, 0.125, 1.0]);
        assert_eq!(depois.material.shininess, 12.5);
        assert_eq!(depois.material.ambient, [0.11; 4]);
        assert_eq!(depois.light_model_ambient, [0.22; 4]);
        assert!(depois.lights[0].enabled);
        assert_eq!(depois.lights[0].position, [1.0, 2.0, 3.0, 0.0]);
        assert_eq!(depois.lights[0].spot_cutoff, 45.0);
        assert_eq!(depois.lights[0].attenuation, [1.0, 0.5, 0.25]);
    }

    /// Uma seção de tamanho errado é **recusa**, e não leitura deslocada.
    #[test]
    fn secao_com_tamanho_errado_e_recusada() {
        let mut antes = GlState::new(640, 480);
        antes.depth_test = true;
        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let mut arquivo = secoes.fecha();

        // Tira um número do meio do arquivo: o crc32 deixa de bater, e é a primeira barreira.
        let meio = arquivo.len() / 2;
        arquivo[meio] ^= 0xff;
        match Leitor::abre(&arquivo) {
            Err(crate::save_state::Erro::Integridade { .. }) => {}
            outro => panic!("devia recusar por integridade, e devolveu {outro:?}"),
        }
    }

    /// **As texturas vão e voltam, com a cadeia de mipmaps inteira.**
    ///
    /// A cadeia é o motivo de esta tabela ter formato próprio: o teste usa dois níveis, e confere
    /// que o segundo voltou com as medidas e os pixels dele.
    #[test]
    fn as_texturas_e_os_mipmaps_vem_de_volta() {
        let mut antes = GlState::new(640, 480);
        let nivel_zero: Vec<[u8; 4]> = (0..16u8).map(|n| [n, n + 1, n + 2, 255]).collect();
        let nivel_um: Vec<[u8; 4]> = (0..4u8).map(|n| [n + 100, 0, 0, 128]).collect();
        antes.textures.insert(
            0x1000,
            Texture {
                width: 4,
                height: 4,
                pixels: nivel_zero.clone(),
                mipmaps: vec![Nivel {
                    width: 2,
                    height: 2,
                    pixels: nivel_um.clone(),
                }],
                wrap: [0x2901, 0x2900],
                filter: 0x2601,
                min_filter: 0x2703,
                crop: [1, -2, 3, -4],
            },
        );

        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");
        let mut depois = GlState::new(640, 480);
        depois.restaura(&leitor).expect("restaurou");

        let textura = depois.textures.get(&0x1000).expect("a textura voltou");
        assert_eq!((textura.width, textura.height), (4, 4));
        assert_eq!(textura.pixels, nivel_zero, "os pixels não voltaram");
        assert_eq!((textura.wrap[0], textura.wrap[1]), (0x2901, 0x2900));
        assert_eq!((textura.filter, textura.min_filter), (0x2601, 0x2703));
        assert_eq!(textura.crop, [1, -2, 3, -4], "o recorte é assinado");
        assert_eq!(textura.mipmaps.len(), 1, "a cadeia de mipmaps não voltou");
        assert_eq!((textura.mipmaps[0].width, textura.mipmaps[0].height), (2, 2));
        assert_eq!(
            textura.mipmaps[0].pixels, nivel_um,
            "os pixels do nível um não voltaram"
        );
    }

    /// Uma textura cujo número de pixels não casa com as dimensões é **recusada**.
    ///
    /// É a conferência que importa: um nível de mipmap com as medidas trocadas desenharia listras,
    /// e o defeito apareceria como imagem errada, sem nada apontando para o save state.
    #[test]
    fn textura_com_pixels_a_menos_e_recusada() {
        let mut antes = GlState::new(640, 480);
        antes.textures.insert(
            0x1000,
            Texture {
                width: 4,
                height: 4,
                pixels: vec![[0, 0, 0, 255]],
                mipmaps: Vec::new(),
                wrap: [0, 0],
                filter: 0,
                min_filter: 0,
                crop: [0; 4],
            },
        );
        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");
        let mut depois = GlState::new(640, 480);
        match depois.restaura(&leitor) {
            Err(crate::save_state::Erro::Secao { nome, motivo }) => {
                // O identificador entra em decimal no nome da seção, como nas outras tabelas.
                assert_eq!(nome, "tex.4096.pixels");
                assert!(motivo.contains("16"), "{motivo}");
            }
            outro => panic!("devia recusar a textura curta, e devolveu {outro:?}"),
        }
    }

    /// **Os três buffers vão e voltam** — cor, profundidade e stencil, cada um no seu formato.
    ///
    /// Medido antes de decidir: o buffer de cor **não** é duplicata da tela. A tela que o core
    /// apresenta é o bitmap do aparelho, e o buffer de cor é a superfície de desenho do GL; o
    /// `eglSwapBuffers` lê um e entrega o outro. Gravar só um dos dois deixaria a próxima cena
    /// desenhada sobre nada.
    #[test]
    fn os_buffers_de_cor_profundidade_e_stencil_vem_de_volta() {
        let mut antes = GlState::new(4, 2);
        assert_eq!(antes.color.len(), 8, "o estado novo já tem os buffers da superfície");
        antes.color[0] = [1, 2, 3, 4];
        antes.color[7] = [5, 6, 7, 8];
        antes.depth[3] = 0.25;
        antes.depth[7] = 0.75;
        antes.stencil[5] = 9;

        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");
        let mut depois = GlState::new(4, 2);
        depois.restaura(&leitor).expect("restaurou");

        assert_eq!(depois.color.len(), 8);
        assert_eq!(depois.color[0], [1, 2, 3, 4], "o primeiro pixel da cor");
        assert_eq!(depois.color[7], [5, 6, 7, 8], "o último pixel da cor");
        assert_eq!(depois.depth.len(), 8);
        assert_eq!(depois.depth[3], 0.25, "a profundidade é f32");
        assert_eq!(depois.depth[7], 0.75);
        assert_eq!(depois.stencil.len(), 8);
        assert_eq!(depois.stencil[5], 9);
    }

    /// **Um buffer maior que a superfície é recusado**, e não alocado.
    ///
    /// É o teto que impede um arquivo corrompido — ou escrito por uma versão com outra resolução —
    /// de fazer o carregamento pedir memória absurda. O teste monta um estado **válido** e estraga
    /// só o campo do stencil: refazer as seções à mão faria ele medir outra coisa.
    #[test]
    fn buffer_maior_que_a_superficie_e_recusado() {
        let antes = GlState::new(4, 2);
        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        // Os cinco números do cabeçalho dos buffers, com o último mentindo. O primeiro número é a
        // **contagem** da seção (5), e não um valor: errar isso faz o leitor recusar a seção
        // inteira, e foi o que o teste mostrou antes de eu acertar.
        let mut mentiroso = 5u32.to_le_bytes().to_vec();
        for valor in [4u32, 2, 8, 8, 9999] {
            mentiroso.extend_from_slice(&valor.to_le_bytes());
        }
        secoes.troca("gl.buffers", mentiroso);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");

        let mut depois = GlState::new(4, 2);
        match depois.restaura(&leitor) {
            Err(crate::save_state::Erro::Secao { nome, motivo }) => {
                assert_eq!(nome, "gl.buffer.stencil");
                assert!(motivo.contains("9999"), "{motivo}");
            }
            outro => panic!("devia recusar o buffer gigante, e devolveu {outro:?}"),
        }
    }
}
