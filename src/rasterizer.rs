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

use crate::gles;

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
}

impl Default for Vertex {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0, 1.0],
            color: [1.0; 4],
            uv: [0.0; 2],
        }
    }
}

/// Uma textura carregada por `TexImage2D`, sempre convertida para RGBA de 8 bits.
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
    /// Como tratar coordenadas fora de `[0, 1)` em cada eixo.
    pub wrap: [u32; 2],
    /// Filtro de ampliação. O de redução também vem por aqui, mas sem mipmap os dois fariam a
    /// mesma coisa, e é o de ampliação que o jogo enxerga.
    pub filter: u32,
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
            // Os padrões do OpenGL ES: repetir nos dois eixos e ampliar por interpolação.
            wrap: [gles::GL_REPEAT; 2],
            filter: gles::GL_LINEAR,
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

    /// A cor de um texel, já normalizada.
    fn texel(&self, x: i32, y: i32) -> [f32; 4] {
        let x = Self::wrap(self.wrap[0], x, self.width);
        let y = Self::wrap(self.wrap[1], y, self.height);
        let p = self.pixels[y * self.width + x];
        std::array::from_fn(|i| p[i] as f32 / 255.0)
    }

    /// Amostra a textura na coordenada dada, com o filtro que o jogo pediu.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [1.0; 4];
        }
        // O texel `n` cobre de `n` a `n+1`, e o centro dele está em `n + 0.5`: daí o meio
        // texel que separa a coordenada contínua da grade de amostras.
        let (x, y) = (u * self.width as f32 - 0.5, v * self.height as f32 - 0.5);
        if self.filter == gles::GL_NEAREST {
            return self.texel(x.round() as i32, y.round() as i32);
        }
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let corners = [
            self.texel(x0, y0),
            self.texel(x0 + 1, y0),
            self.texel(x0, y0 + 1),
            self.texel(x0 + 1, y0 + 1),
        ];
        std::array::from_fn(|c| {
            let top = corners[0][c] + (corners[1][c] - corners[0][c]) * fx;
            let bottom = corners[2][c] + (corners[3][c] - corners[2][c]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// Estado completo do OpenGL ES que o rasterizador mantém.
pub struct GlState {
    pub width: usize,
    pub height: usize,
    /// Cor do quadro, em RGBA de 8 bits — convertida para RGB565 só na apresentação.
    pub color: Vec<[u8; 4]>,
    /// Profundidade normalizada em `[0, 1]`.
    pub depth: Vec<f32>,

    matrix_mode: u32,
    modelview: Vec<Matrix>,
    projection: Vec<Matrix>,
    texture_matrix: Vec<Matrix>,

    viewport: (i32, i32, i32, i32),
    surface: Option<(usize, usize)>,
    clear_color: [f32; 4],
    clear_depth: f32,
    current_color: [f32; 4],

    pub textures: HashMap<u32, Texture>,
    bound_texture: u32,
    texture_env: u32,

    texture_2d: bool,
    /// Unidade de textura ativa, contada de zero. O `glActiveTexture` a escolhe.
    ///
    /// O pipeline lê uma textura por fragmento, então só a unidade zero tem efeito e as outras
    /// são ignoradas. Ignorar não é o mesmo que não ter multitextura: é a diferença entre
    /// desenhar a camada base e desenhar branco. O Resident Evil 4 monta o mundo com duas
    /// unidades e termina cada bloco na unidade 1; como o `glActiveTexture` não era tratado,
    /// tudo caía num estado só, a última ligação vencia e a textura base era perdida — a vila
    /// inteira saía branca.
    active_unit: u32,
    /// Unidade escolhida pelo `glClientActiveTexture`, que vale para o vetor de coordenadas.
    client_unit: u32,
    depth_test: bool,
    depth_mask: bool,
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
    cull_face: bool,
    cull_mode: u32,
    front_face: u32,

    /// Lote de triângulos da draw call em curso. Vive na struct só para reaproveitar a
    /// alocação de uma chamada para a outra.
    batch: Batch,
    /// O que já foi desenhado neste quadro e ainda não virou pixel. Ver [`GlState::flush`].
    pending: Pending,
    /// Os vértices já transformados da draw call em curso. Vive aqui pelo mesmo motivo: o
    /// Quake faz meio milhão de draw calls em vinte e cinco segundos de jogo, e uma alocação
    /// por chamada é meio milhão de idas ao alocador para desenhar dois triângulos de cada vez.
    transformed: Vec<Vertex>,
}

impl GlState {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            color: vec![[0, 0, 0, 255]; width * height],
            depth: vec![1.0; width * height],
            matrix_mode: gles::GL_MODELVIEW,
            modelview: vec![IDENTITY],
            projection: vec![IDENTITY],
            texture_matrix: vec![IDENTITY],
            viewport: (0, 0, width as i32, height as i32),
            surface: None,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            clear_depth: 1.0,
            current_color: [1.0; 4],
            textures: HashMap::new(),
            bound_texture: 0,
            texture_env: gles::GL_MODULATE,
            texture_2d: false,
            active_unit: 0,
            client_unit: 0,
            depth_test: false,
            depth_mask: true,
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

    /// A mesma pergunta para o vetor de coordenadas.
    pub fn base_client_unit(&self) -> bool {
        self.client_unit == 0
    }

    pub fn set_capability(&mut self, capability: u32, on: bool) {
        match capability {
            // Ligar e desligar textura é por unidade, e só a base conta.
            gles::GL_TEXTURE_2D if self.active_unit == 0 => self.texture_2d = on,
            gles::GL_TEXTURE_2D => {}
            gles::GL_DEPTH_TEST => self.depth_test = on,
            gles::GL_BLEND => self.blend = on,
            gles::GL_ALPHA_TEST => self.alpha_test = on,
            gles::GL_CULL_FACE => self.cull_face = on,
            // O recorte por tesoura ainda não existe; ignorá-lo desenha demais, nunca de
            // menos, e é o erro menos visível dos dois.
            gles::GL_SCISSOR_TEST => {}
            _ => {}
        }
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

    pub fn set_texture_env(&mut self, mode: u32) {
        if self.active_unit != 0 {
            return;
        }
        self.texture_env = mode;
    }

    pub fn bind_texture(&mut self, name: u32) {
        if self.active_unit != 0 {
            return;
        }
        self.bound_texture = name;
        // O nome passa a existir já no `BindTexture`: o jogo costuma ajustar os parâmetros
        // antes de mandar os pixels, e sem a entrada esses ajustes se perderiam.
        self.textures.entry(name).or_default();
    }

    /// `glTexParameter` na textura ligada.
    pub fn set_texture_parameter(&mut self, name: u32, value: u32) {
        if self.active_unit != 0 {
            return;
        }
        let Some(texture) = self.textures.get_mut(&self.bound_texture) else {
            return;
        };
        match name {
            gles::GL_TEXTURE_WRAP_S => texture.wrap[0] = value,
            gles::GL_TEXTURE_WRAP_T => texture.wrap[1] = value,
            // Sem mipmap, o filtro de redução com mipmap se comporta como o de base, e o que
            // importa é distinguir vizinho mais próximo de interpolação.
            gles::GL_TEXTURE_MAG_FILTER | gles::GL_TEXTURE_MIN_FILTER => {
                texture.filter = if value == gles::GL_NEAREST {
                    gles::GL_NEAREST
                } else {
                    gles::GL_LINEAR
                };
            }
            _ => {}
        }
    }

    /// `glTexParameteriv(GL_TEXTURE_CROP_RECT_OES, …)` na textura ligada.
    pub fn set_texture_crop(&mut self, crop: [i32; 4]) {
        if self.active_unit != 0 {
            return;
        }
        if let Some(texture) = self.textures.get_mut(&self.bound_texture) {
            texture.crop = crop;
        }
    }

    pub fn bound_texture(&self) -> u32 {
        self.bound_texture
    }

    pub fn clear(&mut self, mask: u32) {
        // Limpar é uma operação sobre o quadro e entra na fila de ordem como qualquer desenho:
        // o que veio antes precisa estar pintado, senão apagaria o que ainda nem existe.
        self.flush();
        if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
            let c = pack(self.clear_color);
            self.color.fill(c);
        }
        if mask & gles::GL_DEPTH_BUFFER_BIT != 0 {
            self.depth.fill(self.clear_depth);
        }
    }

    /// Desenha uma sequência de vértices no modo pedido.
    pub fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
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
        let mut clip = std::mem::take(&mut self.transformed);
        clip.clear();
        clip.extend(vertices.iter().map(|v| {
            // `q` é o quarto componente da coordenada de textura; a divisão por ele é o
            // que permite projeção na textura, e vale 1 no caso comum.
            let [s, t, _, q] = transform(&texture_matrix, [v.uv[0], v.uv[1], 0.0, 1.0]);
            let scale = if q == 0.0 { 1.0 } else { 1.0 / q };
            Vertex {
                position: transform(&mvp, v.position),
                uv: [s * scale, t * scale],
                ..*v
            }
        }));

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
        let (vx, vy, vw, vh) = self.viewport;
        if vw <= 0 || vh <= 0 {
            return;
        }
        let vertices: Vec<Vertex> = corners
            .iter()
            .map(|&([sx, sy], uv)| Vertex {
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
        let (vx, vy, vw, vh) = self.viewport;
        if vw <= 0 || vh <= 0 {
            return;
        }
        // Divisão pela perspectiva e mapeamento para a tela. Guardamos `1/w` para corrigir a
        // interpolação depois: interpolar `u` direto na tela distorce a textura.
        let mut screen = [[0.0f32; 4]; 3];
        for (slot, vertex) in screen.iter_mut().zip(tri.iter()) {
            let [x, y, z, w] = vertex.position;
            let inv_w = 1.0 / w;
            *slot = [
                vx as f32 + (x * inv_w * 0.5 + 0.5) * vw as f32,
                vy as f32 + (0.5 - y * inv_w * 0.5) * vh as f32,
                z * inv_w * 0.5 + 0.5,
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
        if min_x >= max_x || min_y >= max_y {
            return;
        }

        // Os atributos viajam divididos por `w` — é essa divisão que corrige a perspectiva —,
        // e cada um volta multiplicado pelo `w` interpolado. Fazer a divisão aqui, uma vez por
        // vértice, tira três multiplicações por fragmento de dentro do laço.
        let mut over_w = [[0.0f32; 6]; 3];
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

        batch.cost += (max_x - min_x) as usize * (max_y - min_y) as usize;
        batch.triangles.push(Prepared {
            screen,
            over_w,
            inv_area,
            step,
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
            depth_test: self.depth_test,
            depth_mask: self.depth_mask,
            depth_func: self.depth_func,
            blend: self.blend,
            blend_src: self.blend_src,
            blend_dst: self.blend_dst,
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
        // Separar os campos permite ler as texturas e escrever no quadro ao mesmo tempo.
        let Self {
            color,
            depth,
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
            for (color, depth) in color
                .chunks_mut(rows * width)
                .zip(depth.chunks_mut(rows * width))
            {
                let band_top = top;
                top += (color.len() / width) as i32;
                let textures = &*textures;
                scope.spawn(move || {
                    let mut band = Band {
                        top: band_top,
                        color,
                        depth,
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
    /// A superfície em que o jogo desenha costuma ser menor que a tela — o Quake do Zeebo
    /// desenha em 320×400 numa tela de 640×480 —, então a apresentação amplia. É o que a
    /// extensão de escala da Qualcomm faz no console, e sem isso o quadro aparece encolhido
    /// num canto.
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

/// Altura mínima de uma faixa. Mais fina que isto e quase todo triângulo cruza fronteira: o
/// preparo por faixa passa a pesar mais que o pedaço de trabalho que ela ganha — medindo numa
/// máquina de 24 núcleos, dividir 480 linhas em mais de doze faixas já não rendia nada.
const MIN_BAND_ROWS: usize = 40;

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
    over_w: [[f32; 6]; 3],
    inv_area: f32,
    /// Quanto cada função de aresta anda a cada pixel para a direita.
    step: [f32; 3],
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
}

/// Uma draw call à espera de virar pixel: a faixa dela na fila de triângulos, mais o estado do
/// OpenGL que valia quando foi montada.
struct Job {
    first: usize,
    last: usize,
    /// Nome da textura ligada, ou `None` se o desenho não usa textura.
    texture: Option<u32>,
    texture_env: u32,
    depth_test: bool,
    depth_mask: bool,
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
}

impl Job {
    /// Resolve o nome da textura e monta o estado que o preenchimento lê.
    fn uniforms<'a>(&self, textures: &'a HashMap<u32, Texture>, width: usize) -> Uniforms<'a> {
        Uniforms {
            width,
            texture: self.texture.and_then(|name| textures.get(&name)),
            texture_env: self.texture_env,
            depth_test: self.depth_test,
            depth_mask: self.depth_mask,
            depth_func: self.depth_func,
            blend: self.blend,
            blend_src: self.blend_src,
            blend_dst: self.blend_dst,
            alpha_test: self.alpha_test,
            alpha_func: self.alpha_func,
            alpha_ref: self.alpha_ref,
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
    texture_env: u32,
    depth_test: bool,
    depth_mask: bool,
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
}

/// Uma faixa horizontal do quadro: o pedaço exclusivo de uma thread.
struct Band<'a> {
    /// Linha da tela que corresponde ao começo da faixa.
    top: i32,
    color: &'a mut [[u8; 4]],
    depth: &'a mut [f32],
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
            if uniforms.depth_test && !compare(uniforms.depth_func, z, band.depth[index]) {
                continue;
            }

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

            let mut source = [attribute(0), attribute(1), attribute(2), attribute(3)];
            if let Some(texture) = uniforms.texture {
                let texel = texture.sample(attribute(4), attribute(5));
                source = combine(uniforms.texture_env, source, texel);
            }

            if uniforms.alpha_test && !compare(uniforms.alpha_func, source[3], uniforms.alpha_ref) {
                continue;
            }

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

            band.color[index] = pack(mixed);
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
fn clip_near(a: Vertex, b: Vertex) -> Vertex {
    let (da, db) = (a.position[2] + a.position[3], b.position[2] + b.position[3]);
    let t = da / (da - db);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Vertex {
        position: std::array::from_fn(|i| lerp(a.position[i], b.position[i])),
        color: std::array::from_fn(|i| lerp(a.color[i], b.color[i])),
        uv: std::array::from_fn(|i| lerp(a.uv[i], b.uv[i])),
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
fn combine(mode: u32, source: [f32; 4], texel: [f32; 4]) -> [f32; 4] {
    match mode {
        gles::GL_REPLACE => texel,
        gles::GL_DECAL => {
            let mut out = source;
            for c in 0..3 {
                out[c] = source[c] * (1.0 - texel[3]) + texel[c] * texel[3];
            }
            out
        }
        gles::GL_ADD => {
            let mut out = source;
            for c in 0..3 {
                out[c] = (source[c] + texel[c]).min(1.0);
            }
            out[3] = source[3] * texel[3];
            out
        }
        // `GL_MODULATE` é o padrão e o que os jogos usam quase sempre.
        _ => std::array::from_fn(|c| source[c] * texel[c]),
    }
}

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
    use super::*;

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
}
