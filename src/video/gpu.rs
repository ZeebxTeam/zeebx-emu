//! O rasterizador na placa: o mesmo pipeline, com o preenchimento feito pelo OpenGL do host.
//!
//! **Isto não é uma reimplementação do pipeline fixo do GLES1.** Ele não seria necessário: o
//! [`GlState`] já faz toda a etapa de vértice na CPU — modelo-visão, projeção, matriz de textura
//! e iluminação por vértice —, e quando os triângulos chegam ao preenchimento os vértices estão
//! em espaço de recorte com cor e `uv` finais. É exatamente o que uma placa espera receber.
//!
//! Então este backend **contém** um `GlState`, usa-o para toda a contabilidade de estado e para a
//! etapa de vértice, e substitui só o que a medição apontou como caro: o preenchimento. Ter uma
//! fonte única para matrizes e luz é o que torna a comparação entre os dois honesta — se a luz
//! estiver errada, estará errada igual nos dois, e o que sobrar de diferença é do preenchimento.
//!
//! Um efeito colateral bem-vindo: as ~100 mil chamadas de `glEnable`/`glDisable` que a Z-Wheel e
//! o Crash fazem deixam de ser chamadas de GL uma a uma. O estado é aplicado **uma vez por draw**,
//! a partir do que foi anotado aqui.

// A criação do contexto **nosso** é da feature `gpu`, que traz o glutin. Com só a `gl`, o backend
// desenha no contexto que quem chama entregou — que é o caso do core Libretro, e a razão de as
// duas features existirem separadas.
#[cfg(feature = "gpu")]
use super::contexto::Contexto;

/// O contexto próprio, quando esta construção sabe abrir um.
///
/// Com só a feature `gl` o tipo é a unidade: o campo existe e fica vazio, e a inferência de tipos
/// do `match` abaixo não precisa de anotação em dois ramos que só existem em um deles.
#[cfg(feature = "gpu")]
type ContextoProprio = Contexto;
#[cfg(not(feature = "gpu"))]
type ContextoProprio = ();
use super::gles;
use super::rasterizer::{
    GlState, Matrix, QuadroNaPlaca, Rasterizador, TexEnv, Texture as TexturaSalva,
    UnidadeDeTextura, Vertex,
};
use glow::{self, HasContext};
use std::collections::HashMap;

/// `GL_TEXTURE_MAX_ANISOTROPY` e o máximo que a placa aceita, da extensão
/// `EXT_texture_filter_anisotropic` (núcleo no OpenGL 4.6).
const TEXTURE_MAX_ANISOTROPY: u32 = 0x84fe;
const MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84ff;

/// Quantos `f32` cada vértice ocupa no buffer: posição, cor e coordenada de textura.
const FLOATS_POR_VERTICE: usize = 4 + 4 + 2 + 1 + 2;

/// Quantos vértices cabem no anel do buffer de vértices. Ver [`GpuState::anel`].
const VERTICES_NO_ANEL: usize = 1 << 16;

/// Quantos vértices um lote junta antes de ir à placa mesmo sem mudança de estado.
const VERTICES_NO_LOTE: usize = 1 << 14;

/// O que a placa precisa saber de uma textura do jogo, além dos pixels que já estão nela.
struct Textura {
    objeto: glow::Texture,
    largura: usize,
    altura: usize,
    /// O maior nível de mipmap já enviado. Um filtro que peça mipmap sem a cadeia completa
    /// desenha preto no OpenGL — então, quando só existe o nível zero, o filtro é rebaixado.
    maior_nivel: u32,
    crop: [i32; 4],
    filtro: u32,
    filtro_min: u32,
    wrap: [u32; 2],
}

/// O estado do preenchimento, anotado das chamadas e aplicado uma vez por draw.
///
/// É também a chave do lote: desenhos seguidos com o mesmo estado vão juntos à placa. Ver
/// [`GpuState::lote`].
#[derive(Clone, PartialEq)]
struct Estado {
    teste_profundidade: bool,
    mascara_profundidade: bool,
    /// O `glDepthRange`, `(perto, longe)`.
    faixa_profundidade: (f32, f32),
    /// A névoa no momento do desenho. O fator por vértice vem da etapa de vértice; aqui ficam
    /// só o "está ligada" e a cor, que o shader de fragmento lê.
    neblina: crate::video::rasterizer::Neblina,
    func_profundidade: u32,
    mistura: bool,
    mistura_src: u32,
    mistura_dst: u32,
    teste_alfa: bool,
    func_alfa: u32,
    ref_alfa: f32,
    mascara_cor: [bool; 4],
    descarte: bool,
    modo_descarte: u32,
    face_frontal: u32,
    teste_stencil: bool,
    func_stencil: u32,
    ref_stencil: i32,
    mascara_valor_stencil: u32,
    mascara_escrita_stencil: u32,
    op_stencil: [u32; 3],
    env_textura: TexEnv,
    /// A unidade de textura 1, espelhada do estado de software.
    unidade1: UnidadeDeTextura,
    textura_ligada: u32,
    texturando: bool,
    /// A viewport como o jogo a passou, com o `y` de baixo para cima.
    viewport: (i32, i32, i32, i32),
    /// Uma viewport interna já contada do topo, que não passa pela conversão — a do
    /// [`GpuState::import_rgb565_changes`]. Ver [`GpuState::viewport_do_topo`].
    viewport_do_topo_fixa: Option<(i32, i32, i32, i32)>,
    /// O `glScissor` como o jogo o passou, e se o `GL_SCISSOR_TEST` está ligado.
    tesoura: (i32, i32, i32, i32),
    tesoura_ligada: bool,
    limpa_cor: [f32; 4],
    limpa_profundidade: f32,
    limpa_stencil: i32,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            teste_profundidade: false,
            mascara_profundidade: true,
            faixa_profundidade: (0.0, 1.0),
            neblina: crate::video::rasterizer::Neblina::default(),
            func_profundidade: gles::GL_LESS,
            mistura: false,
            mistura_src: gles::GL_ONE,
            mistura_dst: gles::GL_ZERO,
            teste_alfa: false,
            func_alfa: gles::GL_ALWAYS,
            ref_alfa: 0.0,
            mascara_cor: [true; 4],
            descarte: false,
            modo_descarte: gles::GL_BACK,
            face_frontal: gles::GL_CCW,
            teste_stencil: false,
            func_stencil: gles::GL_ALWAYS,
            ref_stencil: 0,
            mascara_valor_stencil: u32::MAX,
            mascara_escrita_stencil: u32::MAX,
            op_stencil: [gles::GL_KEEP; 3],
            env_textura: TexEnv::default(),
            unidade1: UnidadeDeTextura::default(),
            textura_ligada: 0,
            texturando: false,
            viewport: (0, 0, 0, 0),
            viewport_do_topo_fixa: None,
            tesoura: (0, 0, 0, 0),
            tesoura_ligada: false,
            limpa_cor: [0.0, 0.0, 0.0, 1.0],
            limpa_profundidade: 1.0,
            limpa_stencil: 0,
        }
    }
}

/// Se o `glBlitFramebuffer` desta placa entrega o que promete.
///
/// **O anúncio não basta.** O Flycast, que roda nos drivers ruins de Android e de portátil, não
/// confia nele: faz um blit de verdade e, quando o resultado não bate, desenha um quadrilátero no
/// lugar. Aqui o blit também não é enfeite — é o `resolve` do MSAA e a redução do quadro grande
/// antes da leitura —, e um blit que mente **não dá erro**: dá imagem errada em silêncio, que é
/// exatamente o tipo de defeito que só aparece no aparelho de quem está jogando.
///
/// A prova é mínima de propósito: 2×2, uma cor chapada, ida e volta pela CPU. Custa duas texturas
/// e dois framebuffers, uma vez por contexto.
#[cfg(not(target_arch = "wasm32"))]
fn blit_serve(gl: &glow::Context) -> bool {
    use glow::HasContext;

    /// A cor que se escreve num lado e se cobra do outro.
    const MARCA: [u8; 4] = [0x00, 0xff, 0x00, 0xff];

    (|| -> Option<bool> {
        unsafe {
            let criar = |largura: u32| -> Option<(glow::Texture, glow::Framebuffer)> {
                let cor = gl.create_texture().ok()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(cor));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    largura as i32,
                    largura as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                let fbo = gl.create_framebuffer().ok()?;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(cor),
                    0,
                );
                Some((cor, fbo))
            };

            let (cor_a, fbo_a) = criar(2)?;
            let (cor_b, fbo_b) = criar(2)?;

            // Ida: pinta o lado A. A tesoura fica desligada porque ela é do jogo e pode estar
            // recortando fora deste pedaço.
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo_a));
            gl.clear_color(
                f32::from(MARCA[0]) / 255.0,
                f32::from(MARCA[1]) / 255.0,
                f32::from(MARCA[2]) / 255.0,
                f32::from(MARCA[3]) / 255.0,
            );
            gl.clear(glow::COLOR_BUFFER_BIT);

            // Volta: copia A para B pelo mesmo caminho que o `resolve` e a redução usam.
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo_a));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(fbo_b));
            gl.blit_framebuffer(
                0,
                0,
                2,
                2,
                0,
                0,
                2,
                2,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo_b));
            let mut lido = [0u8; 4];
            gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut lido)),
            );

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.delete_framebuffer(fbo_a);
            gl.delete_framebuffer(fbo_b);
            gl.delete_texture(cor_a);
            gl.delete_texture(cor_b);

            // **Um erro armado conta como falha.** Um blit que devolve a imagem certa e deixa um
            // erro pendente é um driver que aceitou por sorte, e o erro apareceria depois, longe
            // daqui, na primeira chamada que o consumisse.
            let limpo = gl.get_error() == glow::NO_ERROR;
            Some(lido == MARCA && limpo)
        }
    })()
    .unwrap_or(false)
}

/// O que já está na placa, para não reenviar o que não mudou.
///
/// **Todo campo começa em `None`, e `None` quer dizer "não se sabe".** É o que torna a
/// invalidação trivial: esquecer o espelho é voltar ao valor padrão, e daí tudo é reenviado uma
/// vez — o que [`GpuState::esquece_o_espelho`] faz quando outra pessoa mexe no contexto.
///
/// São dezoito chamadas de GL por lote em [`GpuState::aplica`], e quase sempre são as mesmas
/// dezoito do lote anterior: o mesmo programa, a mesma névoa, a mesma tesoura. Um jogo que desenha
/// centenas de vezes por quadro paga isso centenas de vezes.
#[derive(Clone, Copy, PartialEq)]
struct Espelho {
    viewport: Option<(i32, i32, i32, i32)>,
    tesoura: Option<(i32, i32, i32, i32)>,
    tesoura_ligada: Option<bool>,
    abraco_de_profundidade: Option<bool>,
    teste_de_profundidade: Option<bool>,
    func_profundidade: Option<u32>,
    mascara_profundidade: Option<bool>,
    faixa_profundidade: Option<(f32, f32)>,
    mistura: Option<bool>,
    func_mistura: Option<(u32, u32)>,
    mascara_cor: Option<[bool; 4]>,
    descarte: Option<bool>,
    modo_descarte: Option<u32>,
    face_frontal: Option<u32>,
    teste_de_estencil: Option<bool>,
    func_estencil: Option<(u32, i32, u32)>,
    mascara_estencil: Option<u32>,
    ops_estencil: Option<[u32; 3]>,
}

impl Default for Espelho {
    /// Tudo desconhecido: é o estado "acabei de receber o contexto de outro".
    fn default() -> Self {
        Self {
            viewport: None,
            tesoura: None,
            tesoura_ligada: None,
            abraco_de_profundidade: None,
            teste_de_profundidade: None,
            func_profundidade: None,
            mascara_profundidade: None,
            faixa_profundidade: None,
            mistura: None,
            func_mistura: None,
            mascara_cor: None,
            descarte: None,
            modo_descarte: None,
            face_frontal: None,
            teste_de_estencil: None,
            func_estencil: None,
            mascara_estencil: None,
            ops_estencil: None,
        }
    }
}

impl Espelho {
    /// Marca o valor como enviado e diz se ele **mudou** — isto é, se a chamada é necessária.
    ///
    /// Conta os dois lados de propósito: sem os números não há como saber se o espelho está
    /// poupando chamadas ou apenas repetindo o que já estava lá.
    fn mudou<T: PartialEq + Copy>(
        slot: &mut Option<T>,
        novo: T,
        enviados: &mut u64,
        poupados: &mut u64,
    ) -> bool {
        if *slot == Some(novo) {
            *poupados += 1;
            false
        } else {
            *slot = Some(novo);
            *enviados += 1;
            true
        }
    }
}

/// **A placa que o frontend avisou que morreu**, pelo endereço do `glow::Context`, ou zero.
///
/// O aviso de que um contexto deixou de valer chega de fora do emulador, **no meio de um quadro**,
/// e quem o recebe não pode mais tocar no contexto: os ponteiros de função que ele resolveu já não
/// existem. Um endereço, comparado sem desreferenciar nada, é o que sobra — e basta, porque duas
/// cópias de `Arc` da mesma placa têm o mesmo endereço.
///
/// **Um `AtomicUsize`, e não um cadeado**, porque quem escreve aqui é a thread de vídeo do frontend
/// enquanto o emulador desenha: ver a nota de corrida em `frontends/libretro/src/lib.rs`. Um
/// `Mutex` no caminho do desenho trocaria uma corrida por um travamento.
static PLACA_MORTA: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// O endereço de uma placa: a identidade dela entre quem a criou e quem desenha nela.
pub fn endereco_da_placa(gl: &glow::Context) -> usize {
    gl as *const glow::Context as usize
}

/// O frontend avisou que a placa em `endereco` deixou de valer.
///
/// No `libretro` os dois callbacks dizem isso: o `context_destroy` avisa que o contexto vai morrer
/// e o `context_reset` que ele nasceu de novo — e a ABI é explícita que os objetos de GL do core
/// são inválidos nos dois casos (`libretro.h`: "When context_reset is called, OpenGL resources in
/// the libretro implementation are guaranteed to be invalid"), e que um `context_reset` pode
/// chegar **sem** o `context_destroy`, quando o contexto se perdeu por fora ("resources should
/// just be recreated without any attempt to free old resources").
pub fn a_placa_morreu(endereco: usize) {
    PLACA_MORTA.store(endereco, std::sync::atomic::Ordering::Relaxed);
}

/// A placa em `endereco` nasceu de novo: se a marca de óbito era da placa anterior **naquele mesmo
/// endereço**, ela sai.
///
/// O endereço é do alocador, e o de uma placa nova pode ser o de uma que morreu. Sem esta limpeza
/// a placa nova nasceria marcada como morta e não desenharia nada, em silêncio.
pub fn a_placa_nasceu(endereco: usize) {
    let _ = PLACA_MORTA.compare_exchange(
        endereco,
        0,
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// O que este estado sabe sobre a placa em que desenha: de quem é o contexto, se ele ainda vale e o
/// que ficou ligado nele.
///
/// **Está separado do [`GpuState`] por causa da regra da casa — não manter mudança sem medida.** Um
/// contexto de GL não existe em teste unitário, mas as três decisões abaixo não dependem de driver
/// nenhum, e são elas que a auditoria de GPU apontou erradas (achados 1, 2 e 4):
///
/// - [`Placa::apaga_ao_morrer`]: se o `Drop` chama os `delete_*`. Com a placa morta, **não chama**;
/// - [`Placa::precisa_ligar`]: se o próximo lote religa programa, VAO e VBO, ou reaproveita o cache;
/// - [`Placa::morreu`]: se a placa emprestada acabou — o que pode acontecer **dentro** de um quadro.
struct Placa {
    /// Se o contexto é de outro — a janela, ou o frontend. Nesse caso o estado tem que ser devolvido
    /// depois de cada uso (ver [`GpuState::devolve_o_contexto`]) e pode ser tomado de volta pelo dono.
    de_outro: bool,
    /// O endereço do contexto, para reconhecer o aviso de que ele morreu. Ver [`PLACA_MORTA`].
    endereco: usize,
    /// Se o **programa**, o `vao` e o `vbo` já estão ligados na placa.
    ///
    /// Os três são criados uma vez e nunca trocam, então ligá-los a cada desenho era pagar três
    /// chamadas de driver por lote — e o lote do Quake chega a umas 370 por quadro. Pior: o
    /// `submete_com` **desligava** os dois no fim de cada desenho, e num driver fino de ARM, como o
    /// Mali dos portáteis, desligar programa e VAO é justamente o que revalida mais coisa. Medido no
    /// caminho de placa do RetroArch, antes e depois (commit `e6a436e`).
    ///
    /// **O que pode largar um dos três é outro código desenhando com o mesmo contexto — e há.** O
    /// `Pintor` da janela termina cada pintura com `use_program(None)` e `bind_vertex_array(None)`
    /// **de propósito**, para devolver o contexto ao `egui` (`src/ui/gpu.rs`), e o contexto em que
    /// ele pinta é o mesmo em que o rasterizador desenha (`src/ui/app.rs`, com
    /// `graphics.gpu_rasterizer`). Por isso o cache não é invalidado só pelo contexto refeito: quem
    /// o invalida é [`GpuState::devolve_o_contexto`] e [`GpuState::desenha_no_fbo`].
    ligados: std::cell::Cell<bool>,
}

impl Placa {
    /// A placa deste estado, com o contexto recém-criado e nada ligado nele.
    fn nova(de_outro: bool, endereco: usize) -> Self {
        Self {
            de_outro,
            endereco,
            ligados: std::cell::Cell::new(false),
        }
    }

    /// Se o contexto é de outro. Ver [`GpuState::devolve_o_contexto`].
    fn de_outro(&self) -> bool {
        self.de_outro
    }

    /// Se a placa emprestada deixou de valer.
    ///
    /// Contexto próprio nunca morre antes do dono: quem o fecharia é este mesmo estado, no `Drop`.
    fn morreu(&self) -> bool {
        self.de_outro && self.endereco == PLACA_MORTA.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Se os nomes de GL deste estado podem ser apagados.
    ///
    /// **Com a placa morta, não podem.** Apagar é chamar `delete_*` no driver, e as funções que este
    /// estado guardou morreram com o contexto: no melhor caso o driver anota um erro, no pior o
    /// ponteiro aponta para código que já não existe. Os nomes são **largados**, e quem os apagaria
    /// já não existe. É a regra que o repositório já escreve do outro lado da janela
    /// (`src/ui/app.rs`, no `on_exit`: "soltar depois seria mexer num contexto morto").
    fn apaga_ao_morrer(&self) -> bool {
        !self.morreu()
    }

    /// Se o próximo lote precisa religar programa, VAO e VBO.
    fn precisa_ligar(&self) -> bool {
        !self.ligados.get()
    }

    /// Os três entraram na placa: o próximo lote pode pular as três chamadas.
    fn ligou(&self) {
        self.ligados.set(true);
    }

    /// O contexto saiu das nossas mãos: o que estava ligado nele deixou de ser nosso.
    fn esquece_o_ligado(&self) {
        self.ligados.set(false);
    }
}

pub struct GpuState {
    /// A contabilidade de estado e a etapa de vértice, compartilhadas com o software.
    estado: GlState,
    /// O contexto que **nós** abrimos, quando não havia nenhum.
    ///
    /// Nunca é lido: existe para não ser solto enquanto o backend vive. Soltá-lo destruiria o
    /// contexto de onde vêm as funções de GL que o `gl` acabou de guardar.
    _proprio: Option<ContextoProprio>,
    /// As funções de GL: emprestadas da janela, ou do contexto próprio.
    gl: std::sync::Arc<glow::Context>,
    /// A placa em que este estado desenha: de quem é o contexto, se ele ainda vale e o que ficou
    /// ligado nele. Ver [`Placa`].
    placa: Placa,
    fill: Estado,
    /// O destino: uma textura de cor mais profundidade e stencil juntos.
    quadro: Option<Destino>,
    programa: glow::Program,
    /// Onde fica cada uniforme do programa, e o que foi mandado para ele por último.
    uniformes: Uniformes,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    /// O buffer de vértices é um anel: `(capacidade, próximo livre)`, em vértices.
    ///
    /// Cada desenho grava na faixa seguinte e desenha a partir dela, e só quando o anel enche o
    /// buffer é pedido de novo, inteiro. Redefinir o buffer a cada desenho — o `buffer_data` —
    /// e refazer os ponteiros de atributo custava 4 µs por chamada, e o Quake faz umas 370 por
    /// quadro: era o que o deixava abaixo da velocidade do console, em câmera lenta.
    anel: (usize, usize),
    /// Se os ponteiros de atributo já estão gravados no `vao`. Eles não mudam: o layout do
    /// vértice é um só, e a posição de cada desenho no anel vai no `first` do `draw_arrays`.
    vao_pronto: bool,
    /// **Os desenhos juntados que ainda não foram à placa**, em triângulos soltos, com o estado
    /// e a perspectiva em que foram pedidos.
    ///
    /// O Quake desenha face por face: um `glDrawArrays` em leque para cada polígono, umas 370
    /// por quadro, quase todas seguidas com a mesma textura e o mesmo estado. Cada uma ia à placa
    /// sozinha — estado, uniformes e a chamada do driver, uns 5 µs —, e o jogo não cabia no
    /// quadro de 60 Hz. Juntos, o leque e a faixa viram triângulos e o lote vai numa chamada só,
    /// quando o estado muda ou quando alguém precisa do que já foi desenhado. Ver
    /// [`GpuState::descarrega`].
    lote: Vec<f32>,
    estado_do_lote: Option<(Estado, bool)>,
    /// Os vértices transformados de um desenho, antes de virarem triângulos no lote.
    soltos: Vec<Vertex>,
    /// A textura de apoio do [`GpuState::import_rgb565_changes`].
    ponte: glow::Texture,
    texturas: HashMap<u32, Textura>,
    /// Buffers reaproveitados entre chamadas, para não pedir memória por quadro.
    vertices: Vec<f32>,
    pixels: Vec<u8>,
    /// Se alguma coisa foi desenhada desde a última conversão do quadro.
    sujo: bool,
    /// A resolução interna, em múltiplos do quadro do console. Ver [`Rasterizador::define_escala`].
    escala: usize,
    /// O quadro reduzido ao tamanho do console, de onde saem as leituras quando `escala > 1`.
    reduzido: Option<(glow::Framebuffer, glow::Texture, (usize, usize))>,
    /// Amostras por pixel do antialias (MSAA); 1 é desligado. Ver [`Rasterizador::define_antialias`].
    amostras: usize,
    /// O filtro anisotrópico aplicado às texturas do jogo; 1 é desligado.
    anisotropia: f32,
    /// A proporção pedida para o 3D, largura sobre altura. `None` é o 4:3 do console. Ver
    /// [`GpuState::extra`].
    proporcao: Option<f32>,
    /// Se o lote que vai para a placa foi transformado por uma projeção em perspectiva.
    em_perspectiva: bool,
    /// Se o `glBlitFramebuffer` desta placa passou na prova de [`blit_serve`].
    ///
    /// Falso desliga o antialias e a resolução interna: os dois dependem de copiar entre
    /// framebuffers, e um blit que mente daria imagem errada em silêncio.
    blit_confiavel: bool,
    /// O framebuffer de fora em que desenhar, quando o frontend entrega um. `None` é o próprio.
    ///
    /// `Some(0)` é o framebuffer padrão do frontend — o que o `glow` escreve `None` no `bind`.
    /// Ver [`Rasterizador::desenha_no_fbo`].
    fbo_externo: Option<u32>,
    /// Se os anexos de profundidade e estêncil são descartados depois do quadro. Ver
    /// [`GpuState::define_descarte_de_tiles`].
    descarta_tiles: bool,
    /// O que já está na placa. Ver [`Espelho`].
    espelho: std::cell::Cell<Espelho>,
    /// Chamadas de estado enviadas e poupadas pelo espelho, para conferência.
    envios_de_estado: std::cell::Cell<u64>,
    poupancas_de_estado: std::cell::Cell<u64>,
}

struct Destino {
    fbo: glow::Framebuffer,
    cor: glow::Texture,
    profundidade: glow::Renderbuffer,
    /// Em pixels do console; o anexo tem `(medida.0 + 2 * extra, medida.1) * escala`.
    medida: (usize, usize),
    escala: usize,
    /// As colunas a mais de cada lado, em pixels do console, para a proporção larga.
    extra: usize,
    amostras: usize,
    /// Com antialias, o desenho vai para este framebuffer de várias amostras — cor e a
    /// profundidade acima —, e é resolvido na `cor` antes de qualquer leitura.
    multi: Option<(glow::Framebuffer, glow::Renderbuffer)>,
}

impl Destino {
    /// O framebuffer em que se desenha.
    fn desenho(&self) -> glow::Framebuffer {
        self.multi.map_or(self.fbo, |(fbo, _)| fbo)
    }
}

impl GpuState {
    /// Monta o programa sobre um contexto, ou diz por que não deu.
    ///
    /// `emprestado` é o contexto da janela, quando há uma. **Receber em vez de criar não é
    /// economia, é correção:** o núcleo roda na mesma thread da interface, e um contexto nosso
    /// tornado corrente ali desliga o do eframe — o egui para de pintar e a janela congela,
    /// enquanto o áudio, que é outra thread, segue tocando.
    ///
    /// Sem janela — o `run` da linha de comando, onde a medição é feita — não há o que emprestar
    /// e abrimos o pbuffer.
    pub fn novo(
        largura: usize,
        altura: usize,
        emprestado: Option<std::sync::Arc<glow::Context>>,
    ) -> Result<Self, String> {
        #[allow(unused_variables)]
        let (proprio, gl, emprestado) = match emprestado {
            Some(gl) => (None, gl, true),
            // **Sem contexto emprestado, esta feature não abre um.** Quem precisa de contexto
            // próprio usa a `gpu`; quem recebe o do frontend — o core — não pode abrir um, e
            // responder isso é melhor que falhar com um erro de link.
            #[cfg(feature = "gpu")]
            None => {
                let proprio = Contexto::novo()?;
                let gl = proprio.gl.clone();
                (Some(proprio), gl, false)
            }
            #[cfg(not(feature = "gpu"))]
            None => {
                return Err(
                    "sem contexto emprestado: esta construção não abre contexto de placa".to_string(),
                )
            }
        };
        let (programa, vao, vbo, ponte) = unsafe {
            let programa = compila(&gl)?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let ponte = gl.create_texture()?;
            (programa, vao, vbo, ponte)
        };
        // A prova do blit antes de qualquer desenho, uma vez por contexto.
        #[cfg(not(target_arch = "wasm32"))]
        let blit_confiavel = blit_serve(&gl);
        // O `wasm32` não tem `blitFramebuffer`: ali o WebGL2 não expõe a função, e o caminho de
        // placa nem é usado. Responder `true` mantém o resto do código com uma resposta só.
        #[cfg(target_arch = "wasm32")]
        let blit_confiavel = true;
        if !blit_confiavel {
            crate::registro!(
                crate::registro::Nivel::Aviso,
                "gl",
                "o glBlitFramebuffer desta placa não devolve o que mandaram copiar: antialias e resolução interna ficam desligados nesta sessão"
            );
        }
        let placa = Placa::nova(emprestado, endereco_da_placa(&gl));
        Ok(Self {
            estado: GlState::new(largura, altura),
            _proprio: proprio,
            gl,
            placa,
            blit_confiavel,
            descarta_tiles: false,
            // **A viewport nasce com a tela inteira**, que é o que o OpenGL especifica como
            // padrão e o que o `GlState::new` faz. Nascer em zero era o que apagava toda a
            // geometria da Z-Wheel: ela nunca chama `glViewport` — zero vezes em treze segundos
            // — e ficava com um `glViewport(0, 0, 0, 0)`, de onde nada sai. Só o `import`
            // aparecia, porque ele põe a sua própria.
            fill: Estado {
                viewport: (0, 0, largura as i32, altura as i32),
                tesoura: (0, 0, largura as i32, altura as i32),
                ..Estado::default()
            },
            quadro: None,
            programa,
            uniformes: Uniformes::default(),
            vao,
            vbo,
            anel: (0, 0),
            vao_pronto: false,
            lote: Vec::new(),
            estado_do_lote: None,
            soltos: Vec::new(),
            ponte,
            texturas: HashMap::new(),
            vertices: Vec::new(),
            pixels: Vec::new(),
            sujo: true,
            escala: 1,
            proporcao: None,
            em_perspectiva: false,
            fbo_externo: None,
            espelho: std::cell::Cell::new(Espelho::default()),
            envios_de_estado: std::cell::Cell::new(0),
            poupancas_de_estado: std::cell::Cell::new(0),
            reduzido: None,
            amostras: 1,
            anisotropia: 1.0,
        })
    }

    /// Garante que o destino existe no tamanho do quadro e o deixa ligado.
    /// As colunas a mais de cada lado, em pixels da superfície, para a proporção pedida.
    ///
    /// Só quando a superfície vai à tela inteira: um pbuffer menor que o quadro (a Z-Wheel
    /// desenha em 640×330) não tem "lados" para abrir.
    ///
    /// **A conta é em pixels da superfície.** Na superfície esticada pela Qualcomm cada coluna
    /// dela vale `fw / sw` colunas da tela: o Quake desenha em 320×400, e as colunas a mais do
    /// 16:9 são as da tela divididas por dois. Contadas em pixels da tela, como antes, a imagem
    /// larga saía com o dobro dos lados. E só havia lados quando a superfície era do tamanho do
    /// quadro — no Quake, nunca: o 16:9 não abria nada, e com a resolução interna acima de 1 o
    /// quadro ia à janela na proporção da superfície, estreito e menor.
    fn extra(&self) -> usize {
        let (fw, fh) = self.estado.frame_size();
        let Some(aspecto) = self.proporcao else {
            return 0;
        };
        let (sw, sh) = self.estado.surface();
        let tela_inteira = (sw, sh) == (fw, fh) || self.estado.superficie_esticada();
        if !tela_inteira || fh == 0 || fw == 0 {
            return 0;
        }
        let largura = (fh as f32 * aspecto).round() as usize;
        largura.saturating_sub(fw) / 2 * sw / fw
    }

    /// O framebuffer em que se desenha **agora**.
    ///
    /// Quando o frontend entrega um framebuffer, é ele o alvo — e não o nosso. Este método existe
    /// porque a escolha estava escrita em dois lugares, e os dois discordaram: o caminho que cria o
    /// destino respeitava o framebuffer do frontend, e o caminho curto (destino já pronto, igual)
    /// religava o nosso. Como o curto é o que roda em todo quadro depois do primeiro, o desenho
    /// ficava no nosso framebuffer, o frontend apresentava o dele — tela preta — e a placa
    /// trabalhava o mesmo tanto. O sintoma foi exatamente esse: **placa ocupada e nada na tela**.
    fn alvo_do_desenho(&self) -> Option<glow::Framebuffer> {
        match self.fbo_externo {
            // `0` é o framebuffer que já está ligado; o frontend é quem manda nele.
            Some(0) => None,
            Some(id) => std::num::NonZeroU32::new(id).map(glow::NativeFramebuffer),
            None => self.quadro.as_ref().map(Destino::desenho),
        }
    }

    fn destino(&mut self) {
        let medida = self.estado.frame_size();
        let (escala, amostras, extra) = (self.escala, self.amostras, self.extra());
        if self.quadro.as_ref().is_some_and(|d| {
            d.medida == medida && d.escala == escala && d.amostras == amostras && d.extra == extra
        }) {
            let alvo = self.alvo_do_desenho();
            unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, alvo) };
            return;
        }
        let gl = &self.gl;
        unsafe {
            if let Some(antigo) = self.quadro.take() {
                solta_destino(gl, antigo);
            }
            let (largura, altura) = (
                ((medida.0 + 2 * extra) * escala) as i32,
                (medida.1 * escala) as i32,
            );
            let cor = gl.create_texture().expect("textura de cor");
            gl.bind_texture(glow::TEXTURE_2D, Some(cor));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                largura,
                altura,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            // Profundidade e stencil no mesmo anexo: é a combinação que o OpenGL garante. Com
            // antialias ele tem as mesmas amostras da cor, e fica no framebuffer de desenho.
            let profundidade = gl.create_renderbuffer().expect("buffer de profundidade");
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(profundidade));
            match amostras > 1 {
                true => gl.renderbuffer_storage_multisample(
                    glow::RENDERBUFFER,
                    amostras as i32,
                    glow::DEPTH24_STENCIL8,
                    largura,
                    altura,
                ),
                false => gl.renderbuffer_storage(
                    glow::RENDERBUFFER,
                    glow::DEPTH24_STENCIL8,
                    largura,
                    altura,
                ),
            }
            let fbo = gl.create_framebuffer().expect("framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(cor),
                0,
            );
            let multi = match amostras > 1 {
                false => {
                    gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::DEPTH_STENCIL_ATTACHMENT,
                        glow::RENDERBUFFER,
                        Some(profundidade),
                    );
                    None
                }
                true => {
                    let cor_multi = gl.create_renderbuffer().expect("cor com amostras");
                    gl.bind_renderbuffer(glow::RENDERBUFFER, Some(cor_multi));
                    gl.renderbuffer_storage_multisample(
                        glow::RENDERBUFFER,
                        amostras as i32,
                        glow::RGBA8,
                        largura,
                        altura,
                    );
                    let fbo_multi = gl.create_framebuffer().expect("framebuffer com amostras");
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo_multi));
                    gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::RENDERBUFFER,
                        Some(cor_multi),
                    );
                    gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::DEPTH_STENCIL_ATTACHMENT,
                        glow::RENDERBUFFER,
                        Some(profundidade),
                    );
                    Some((fbo_multi, cor_multi))
                }
            };
            // Um framebuffer novo tem conteúdo indefinido, enquanto os vetores do rasterizador
            // de software nascem em preto opaco, profundidade 1 e stencil 0. Igualar aqui evita
            // que o primeiro quadro dependa do que a placa deixou na memória. O de desenho fica
            // ligado no fim, que é o que quem chama espera.
            gl.viewport(0, 0, largura, altura);
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.stencil_mask(u32::MAX);
            // **Um destino novo mexe no estado por fora do `aplica`.** Daqui em diante o espelho
            // não sabe o que está na placa, e prefere reenviar tudo a mentir.
            self.esquece_o_espelho();
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear_depth_f32(1.0);
            gl.clear_stencil(0);
            for alvo in [Some(fbo), multi.map(|(f, _)| f)].into_iter().flatten() {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(alvo));
                gl.clear(
                    glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT,
                );
            }
            let destino = Destino {
                fbo,
                cor,
                profundidade,
                medida,
                escala,
                extra,
                amostras,
                multi,
            };
            self.quadro = Some(destino);
            // Quem manda no alvo é o frontend, quando ele entregou um framebuffer; sem isso, o
            // destino é o nosso, com o antialias resolvido depois. A escolha é a mesma do caminho
            // curto, e é por isso que ela vive em `alvo_do_desenho`.
            let alvo = self.alvo_do_desenho();
            gl.bind_framebuffer(glow::FRAMEBUFFER, alvo);
        }
    }

    /// Resolve o antialias: as amostras do framebuffer de desenho viram a `cor`.
    fn resolve(&self) {
        let Some(destino) = self.quadro.as_ref() else {
            return;
        };
        let Some((multi, _)) = destino.multi else {
            return;
        };
        let (w, h) = (
            ((destino.medida.0 + 2 * destino.extra) * destino.escala) as i32,
            (destino.medida.1 * destino.escala) as i32,
        );
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(multi));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(destino.fbo));
            gl.disable(glow::SCISSOR_TEST);
            self.esquece_a_tesoura();
            gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(destino.fbo));
        }
    }

    /// A viewport com o `y` contado do topo, que é como o destino é guardado.
    ///
    /// O `glViewport` conta de baixo para cima; ver o mesmo método no rasterizador de software
    /// para o que isso quebrava no Crash Nitro Kart.
    fn viewport_do_topo(&self) -> (i32, i32, i32, i32) {
        if let Some(fixa) = self.fill.viewport_do_topo_fixa {
            return fixa;
        }
        let (x, y, largura, altura) = self.fill.viewport;
        let altura_da_superficie = self.estado.surface().1 as i32;
        (x, altura_da_superficie - y - altura, largura, altura)
    }

    /// Põe na placa o estado anotado. Chamado uma vez por draw.
    /// Copia o estado de desenho do `GlState` para o espelho que vai para o GL.
    ///
    /// Existe por causa do save state. O espelho (`fill`) é atualizado **campo a campo** por cada
    /// método do `Rasterizador`, e não é derivado do `GlState` a cada lote. Carregar um estado troca
    /// o `GlState` inteiro e deixa o espelho descrevendo o mundo anterior — e como é o espelho que
    /// o `aplica` escreve na placa, a cena sairia com a matriz, as bandeiras de teste e as cores de
    /// limpeza de antes. Esta função é o ponto único onde os dois voltam a concordar.
    fn ressincroniza_o_espelho(&mut self) {
        let e = &self.estado;
        self.fill.teste_profundidade = e.depth_test;
        self.fill.mascara_profundidade = e.depth_mask;
        self.fill.faixa_profundidade = e.depth_range;
        self.fill.neblina = e.fog;
        self.fill.func_profundidade = e.depth_func;
        self.fill.mistura = e.blend;
        self.fill.mistura_src = e.blend_src;
        self.fill.mistura_dst = e.blend_dst;
        self.fill.teste_alfa = e.alpha_test;
        self.fill.func_alfa = e.alpha_func;
        self.fill.ref_alfa = e.alpha_ref;
        self.fill.mascara_cor = e.color_mask;
        self.fill.descarte = e.cull_face;
        self.fill.modo_descarte = e.cull_mode;
        self.fill.face_frontal = e.front_face;
        self.fill.teste_stencil = e.stencil_test;
        self.fill.func_stencil = e.stencil_func;
        self.fill.ref_stencil = e.stencil_ref;
        self.fill.mascara_valor_stencil = e.stencil_value_mask;
        self.fill.mascara_escrita_stencil = e.stencil_write_mask;
        self.fill.op_stencil = e.stencil_op;
        self.fill.env_textura = e.texture_env;
        self.fill.unidade1 = e.unidade1();
        self.fill.textura_ligada = e.bound_texture;
        self.fill.texturando = e.texture_2d;
        self.fill.viewport = e.viewport;
        self.fill.viewport_do_topo_fixa = None;
        // O espelho guarda a tesoura **crua** e uma bandeira: ele não tem o estado "sem tesoura",
        // e é a bandeira que o `aplica` consulta para ligá-la ou não.
        self.fill.tesoura = e.tesoura_crua;
        self.fill.tesoura_ligada = e.tesoura_ligada;
        self.fill.limpa_cor = e.clear_color;
        self.fill.limpa_profundidade = e.clear_depth;
        self.fill.limpa_stencil = i32::from(e.clear_stencil);
    }

    /// Refaz os objetos de textura da placa a partir da copia autoritativa do estado GL.
    fn recria_texturas_restauradas(&mut self) {
        let gl = self.gl.clone();
        for (_, textura) in self.texturas.drain() {
            unsafe { gl.delete_texture(textura.objeto) };
        }

        let salvas: Vec<(u32, TexturaSalva)> = self
            .estado
            .textures
            .iter()
            .map(|(&nome, textura)| (nome, textura.clone()))
            .collect();
        unsafe { gl.active_texture(glow::TEXTURE0) };

        for (nome, salva) in salvas {
            if salva.width == 0
                || salva.height == 0
                || salva.pixels.len() < salva.width * salva.height
            {
                continue;
            }
            let Ok(objeto) = (unsafe { gl.create_texture() }) else {
                continue;
            };
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(objeto));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    salva.width as i32,
                    salva.height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(bytes_de_rgba(&salva.pixels))),
                );
            }

            let mut maior_nivel = 0u32;
            for (indice, nivel) in salva.mipmaps.iter().enumerate() {
                if nivel.width == 0
                    || nivel.height == 0
                    || nivel.pixels.len() < nivel.width * nivel.height
                {
                    break;
                }
                let nivel_gl = (indice + 1) as u32;
                unsafe {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        nivel_gl as i32,
                        glow::RGBA8 as i32,
                        nivel.width as i32,
                        nivel.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(bytes_de_rgba(&nivel.pixels))),
                    );
                }
                maior_nivel = nivel_gl;
            }
            unsafe { gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4) };

            self.texturas.insert(
                nome,
                Textura {
                    objeto,
                    largura: salva.width,
                    altura: salva.height,
                    maior_nivel,
                    crop: salva.crop,
                    filtro: salva.filter,
                    filtro_min: salva.min_filter,
                    wrap: salva.wrap,
                },
            );
            if let Some(textura) = self.texturas.get(&nome) {
                self.parametros(textura);
            }
        }
        unsafe { gl.bind_texture(glow::TEXTURE_2D, None) };
    }

    /// Descarta os recursos que representam o quadro do host, nao o estado do guest.
    fn descarta_caches_de_quadro(&mut self) {
        let gl = self.gl.clone();
        // **Com a placa morta, os objetos são largados, não apagados.** Quem chega aqui depois de o
        // contexto acabar — um carregar de estado entre dois quadros, por exemplo — chamaria os
        // `delete_*` de um contexto que já era. Ver [`Placa::apaga_ao_morrer`].
        if self.placa.apaga_ao_morrer() {
            unsafe {
                if let Some(destino) = self.quadro.take() {
                    solta_destino(&gl, destino);
                }
                if let Some((fbo, cor, _)) = self.reduzido.take() {
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(cor);
                }
            }
        } else {
            self.quadro = None;
            self.reduzido = None;
        }
        self.uniformes = Uniformes::default();
        self.anel = (0, 0);
        self.vao_pronto = false;
        // O contexto foi refeito: o que estava ligado nele deixou de estar.
        self.placa.esquece_o_ligado();
        self.lote.clear();
        self.estado_do_lote = None;
        self.soltos.clear();
        self.vertices.clear();
        self.sujo = true;
    }

    /// Esquece tudo o que o espelho sabia: a próxima [`GpuState::aplica`] reenvia o estado
    /// inteiro.
    ///
    /// Chamado quando **outra pessoa** mexe no contexto — o `egui`, o frontend — ou quando o
    /// próprio rasterizador o mexe fora do `aplica` (o `destino` novo, o `resolve`, a leitura do
    /// quadro). Sem isto o espelho mentiria, e mentir aqui é desenho errado.
    fn esquece_o_espelho(&self) {
        self.espelho.set(Espelho::default());
    }

    /// Só a parte da tesoura: o `glScissor` e o `GL_SCISSOR_TEST`.
    ///
    /// É o que basta nos pontos que desligam a tesoura para um blit — ver os comentários em
    /// `resolve`, `liga_para_leitura` e `clear`.
    fn esquece_a_tesoura(&self) {
        let mut m = self.espelho.get();
        m.tesoura = None;
        m.tesoura_ligada = None;
        self.espelho.set(m);
    }

    /// Quantas chamadas de estado o espelho enviou e quantas poupou.
    pub fn estado_enviado_e_poupado(&self) -> (u64, u64) {
        (
            self.envios_de_estado.get(),
            self.poupancas_de_estado.get(),
        )
    }

    fn aplica(&mut self) {
        let (x, y, w, h) = self.viewport_do_topo();
        let extra = self.quadro.as_ref().map_or(0, |d| d.extra) as i32;
        // **Na proporção larga, a perspectiva ganha lados e o resto só vai para o centro.** A
        // viewport de um lote em perspectiva cresce na razão `k` e o `x` de recorte encolhe na
        // mesma razão (em `draw`): o que estava na tela cai no mesmo pixel de antes, deslocado
        // para o centro, e o que ficava fora do recorte aparece nos lados. HUD e 2D, em
        // ortográfica, só se deslocam.
        let superficie = self.estado.surface().0 as i32;
        let para_o_anexo = |x: i32, w: i32| match (self.em_perspectiva, extra > 0) {
            (true, true) => {
                let largura = w * (superficie + 2 * extra) / superficie.max(1);
                (extra + x + w / 2 - largura / 2, largura)
            }
            _ => (x + extra, w),
        };
        let (x, w) = para_o_anexo(x, w);
        let gl = &self.gl;
        let e = &self.fill;
        // **Tudo o que segue passa pelo espelho.** São dezoito chamadas por lote, quase sempre
        // com os mesmos valores do lote anterior.
        let mut m = self.espelho.get();
        let mut e_n = 0u64;
        let mut p_n = 0u64;
        unsafe {
            // A viewport vem em pixels do console; o anexo é `escala` vezes maior.
            let n = self.escala as i32;
            let viewport = (x * n, y * n, w.max(0) * n, h.max(0) * n);
            if Espelho::mudou(&mut m.viewport, viewport, &mut e_n, &mut p_n) {
                gl.viewport(viewport.0, viewport.1, viewport.2, viewport.3);
            }
            // **O rasterizador de software só recorta no plano próximo.** O OpenGL recorta nos
            // seis planos do frustum, e o plano distante fazia superfícies inteiras desaparecerem
            // — na Z-Wheel era uma faixa do fundo, entre a linha do horizonte e o chão. Preso em
            // vez de recortado, o comportamento volta a ser o do software.
            //
            // É core no OpenGL desktop, mas não existe no GLES. Emitir o enum inválido em todo
            // lote custa validação no driver Mali e deixa `GL_INVALID_ENUM` pendente — e é por
            // isso que ele tem um valor próprio aqui, em vez de ir direto para a chamada.
            let embutido = gl.version().is_embedded;
            if !embutido
                && Espelho::mudou(&mut m.abraco_de_profundidade, true, &mut e_n, &mut p_n)
            {
                gl.enable(glow::DEPTH_CLAMP);
            }
            // O `glScissor` do jogo vem em pixels do console, com o `y` de baixo para cima —
            // a mesma convenção da viewport —, e o anexo é `escala` vezes maior. Ver
            // [`tesoura_no_anexo`].
            if Espelho::mudou(&mut m.tesoura_ligada, e.tesoura_ligada, &mut e_n, &mut p_n) {
                liga(gl, glow::SCISSOR_TEST, e.tesoura_ligada);
            }
            if e.tesoura_ligada {
                let (sx, sy, sw, sh) = tesoura_no_anexo(e.tesoura, self.estado.surface(), extra);
                let tesoura = (sx * n, sy * n, sw.max(0) * n, sh.max(0) * n);
                if Espelho::mudou(&mut m.tesoura, tesoura, &mut e_n, &mut p_n) {
                    gl.scissor(tesoura.0, tesoura.1, tesoura.2, tesoura.3);
                }
            }
            if Espelho::mudou(
                &mut m.teste_de_profundidade,
                e.teste_profundidade,
                &mut e_n,
                &mut p_n,
            ) {
                liga(gl, glow::DEPTH_TEST, e.teste_profundidade);
            }
            if Espelho::mudou(
                &mut m.func_profundidade,
                e.func_profundidade,
                &mut e_n,
                &mut p_n,
            ) {
                gl.depth_func(e.func_profundidade);
            }
            if Espelho::mudou(
                &mut m.mascara_profundidade,
                e.mascara_profundidade,
                &mut e_n,
                &mut p_n,
            ) {
                gl.depth_mask(e.mascara_profundidade);
            }
            if Espelho::mudou(
                &mut m.faixa_profundidade,
                e.faixa_profundidade,
                &mut e_n,
                &mut p_n,
            ) {
                gl.depth_range_f32(e.faixa_profundidade.0, e.faixa_profundidade.1);
            }
            if Espelho::mudou(&mut m.mistura, e.mistura, &mut e_n, &mut p_n) {
                liga(gl, glow::BLEND, e.mistura);
            }
            let func_mistura = (e.mistura_src, e.mistura_dst);
            if Espelho::mudou(&mut m.func_mistura, func_mistura, &mut e_n, &mut p_n) {
                gl.blend_func(func_mistura.0, func_mistura.1);
            }
            if Espelho::mudou(&mut m.mascara_cor, e.mascara_cor, &mut e_n, &mut p_n) {
                let [r, g, b, a] = e.mascara_cor;
                gl.color_mask(r, g, b, a);
            }
            if Espelho::mudou(&mut m.descarte, e.descarte, &mut e_n, &mut p_n) {
                liga(gl, glow::CULL_FACE, e.descarte);
            }
            if Espelho::mudou(&mut m.modo_descarte, e.modo_descarte, &mut e_n, &mut p_n) {
                gl.cull_face(e.modo_descarte);
            }
            // A linha 0 do framebuffer é tratada como o topo da imagem, e o Y é virado no shader
            // de vértice. Isso inverte a orientação vista pelo descarte de face, então a face
            // frontal é trocada aqui para compensar — sem isto o descarte come o lado errado.
            let face = match e.face_frontal {
                gles::GL_CCW => glow::CW,
                _ => glow::CCW,
            };
            if Espelho::mudou(&mut m.face_frontal, face, &mut e_n, &mut p_n) {
                gl.front_face(face);
            }
            if Espelho::mudou(
                &mut m.teste_de_estencil,
                e.teste_stencil,
                &mut e_n,
                &mut p_n,
            ) {
                liga(gl, glow::STENCIL_TEST, e.teste_stencil);
            }
            let func_estencil = (e.func_stencil, e.ref_stencil, e.mascara_valor_stencil);
            if Espelho::mudou(&mut m.func_estencil, func_estencil, &mut e_n, &mut p_n) {
                gl.stencil_func(func_estencil.0, func_estencil.1, func_estencil.2);
            }
            if Espelho::mudou(
                &mut m.mascara_estencil,
                e.mascara_escrita_stencil,
                &mut e_n,
                &mut p_n,
            ) {
                gl.stencil_mask(e.mascara_escrita_stencil);
            }
            if Espelho::mudou(&mut m.ops_estencil, e.op_stencil, &mut e_n, &mut p_n) {
                gl.stencil_op(e.op_stencil[0], e.op_stencil[1], e.op_stencil[2]);
            }
        }
        self.espelho.set(m);
        self.envios_de_estado
            .set(self.envios_de_estado.get() + e_n);
        self.poupancas_de_estado
            .set(self.poupancas_de_estado.get() + p_n);
    }

    /// Manda o lote de vértices já transformados para a placa.
    ///
    /// `textura` existe porque a ponte do [`GpuState::import_rgb565_changes`] não pertence ao
    /// jogo e portanto não está no mapa de texturas dele.
    /// Manda à placa o lote em curso, com o estado em que ele foi juntado.
    ///
    /// **Todo mundo que mexe na placa fora do desenho chama isto antes**: quem limpa, quem sobe
    /// ou apaga textura, quem lê o quadro, quem troca o destino. O que foi pedido antes tem de
    /// chegar antes — um `glClear` que passasse na frente do lote apagaria o que o jogo desenhou
    /// antes dele. As mudanças de estado não precisam: elas mudam o `fill`, e o `fill` diferente
    /// já fecha o lote no próximo desenho.
    fn descarrega(&mut self) {
        let Some((estado, perspectiva)) = self.estado_do_lote.take() else {
            return;
        };
        if self.lote.is_empty() {
            return;
        }
        let atual = std::mem::replace(&mut self.fill, estado);
        let anteriores = std::mem::replace(&mut self.vertices, std::mem::take(&mut self.lote));
        self.em_perspectiva = perspectiva;
        self.submete_com(glow::TRIANGLES, -1.0, None);
        self.em_perspectiva = false;
        self.lote = std::mem::replace(&mut self.vertices, anteriores);
        self.lote.clear();
        self.fill = atual;
    }

    fn submete_com(&mut self, modo: u32, virar: f32, textura: Option<glow::Texture>) {
        let quantos = self.vertices.len() / FLOATS_POR_VERTICE;
        if quantos == 0 {
            return;
        }
        // **A placa pode morrer no meio do quadro.** O aviso do frontend não espera o `retro_run`
        // terminar, e desenhar depois dele é chamar funções de GL que já não existem. O que estava
        // na placa fica — o quadro sai do que já foi desenhado —, e a sessão nasce de novo no
        // começo do quadro seguinte, que é onde a perda vira troca de rasterizador.
        if self.placa.morreu() {
            self.vertices.clear();
            return;
        }
        self.destino();
        self.aplica();
        let textura_da_ponte = textura.is_some();
        let textura = textura.or_else(|| {
            self.fill
                .texturando
                .then(|| self.texturas.get(&self.fill.textura_ligada))
                .flatten()
                .map(|t| t.objeto)
        });
        let textura1 = match (textura_da_ponte, self.fill.unidade1.ligada) {
            (false, true) => self.texturas.get(&self.fill.unidade1.textura).map(|t| t.objeto),
            _ => None,
        };
        let gl = &self.gl;
        unsafe {
            // Ligados **uma vez**, e nao a cada desenho: ver [`Placa::ligados`].
            if self.placa.precisa_ligar() {
                gl.use_program(Some(self.programa));
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                self.placa.ligou();
            }
            let passo = (FLOATS_POR_VERTICE * 4) as i32;
            if !self.vao_pronto {
                for (indice, tamanho, deslocamento) in
                    [(0u32, 4i32, 0i32), (1, 4, 16), (2, 2, 32), (3, 1, 40), (4, 2, 44)]
                {
                    gl.enable_vertex_attrib_array(indice);
                    gl.vertex_attrib_pointer_f32(
                        indice,
                        tamanho,
                        glow::FLOAT,
                        false,
                        passo,
                        deslocamento,
                    );
                }
                self.vao_pronto = true;
            }
            let (capacidade, livre) = self.anel;
            if livre + quantos > capacidade {
                // O buffer novo não espera os desenhos que ainda usam o antigo: o driver
                // entrega outra memória, e a de antes é solta quando eles terminam.
                let capacidade = VERTICES_NO_ANEL.max(quantos);
                gl.buffer_data_size(
                    glow::ARRAY_BUFFER,
                    (capacidade * FLOATS_POR_VERTICE * 4) as i32,
                    glow::STREAM_DRAW,
                );
                self.anel = (capacidade, 0);
            }
            let primeiro = self.anel.1;
            gl.buffer_sub_data_u8_slice(
                glow::ARRAY_BUFFER,
                primeiro as i32 * passo,
                bytes_de_f32(&self.vertices),
            );
            self.anel.1 += quantos;
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, textura);
            uniforme_i32(gl, &self.uniformes, self.programa, "amostra", 0);
            uniforme_i32(gl, &self.uniformes, self.programa, "texturando", i32::from(textura.is_some()));
            envia_env(gl, &self.uniformes, self.programa, "", &self.fill.env_textura);
            // A unidade 1 só entra com textura de verdade: ligada sem textura carregada, ela
            // passaria o anterior adiante com um texel preto.
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, textura1);
            gl.active_texture(glow::TEXTURE0);
            uniforme_i32(gl, &self.uniformes, self.programa, "amostra1", 1);
            uniforme_i32(gl, &self.uniformes, self.programa, "texturando1", i32::from(textura1.is_some()));
            if textura1.is_some() {
                envia_env(gl, &self.uniformes, self.programa, "1", &self.fill.unidade1.env);
            }
            uniforme_i32(
                gl,
                &self.uniformes,
                self.programa,
                "func_alfa",
                match self.fill.teste_alfa {
                    true => codigo_alfa(self.fill.func_alfa),
                    false => 7,
                },
            );
            uniforme_f32(gl, &self.uniformes, self.programa, "ref_alfa", self.fill.ref_alfa);
            uniforme_f32(gl, &self.uniformes, self.programa, "virar", virar);
            let neblina = self.fill.neblina;
            uniforme_i32(
                gl,
                &self.uniformes,
                self.programa,
                "com_neblina",
                i32::from(neblina.ligada && neblina.permitida),
            );
            uniforme_vec3(gl, &self.uniformes, self.programa, "cor_neblina", neblina.cor);
            gl.draw_arrays(modo, primeiro as i32, quantos as i32);
            // **Nao desligue aqui.** Desligar no fim de cada desenho era o custo que este cache
            // veio tirar: um programa a menos por lote e uma revalidacao a menos em cada um.
        }
        self.devolve_o_contexto();
        self.sujo = true;
    }

    /// Empilha um vértice no buffer de envio.
    fn poe(&mut self, v: &Vertex) {
        poe_em(&mut self.vertices, v);
    }

    /// Deixa ligado, para leitura, um framebuffer com o quadro no tamanho do console.
    ///
    /// Com `escala` 1 é o próprio destino. Acima disso o quadro grande é reduzido na placa, com
    /// filtro linear, antes de qualquer leitura: é o que o jogo vê — o `GetColorBufferQUALCOMM`, o
    /// `glReadPixels`, a cópia para a tela —, e ler o quadro grande seria mover o quadrado do fator
    /// em bytes para jogar quase tudo fora.
    fn liga_para_leitura(&mut self) {
        self.destino();
        // O framebuffer do frontend não passa pelo nosso resolve. No caminho interno, resolve
        // MSAA antes de ler.
        if self.fbo_externo.is_none() {
            self.resolve();
        }
        let extra = self.quadro.as_ref().map_or(0, |d| d.extra) as i32;
        if self.escala <= 1 && extra == 0 {
            let fbo = match self.fbo_externo {
                Some(_) => self.alvo_do_desenho(),
                None => self.quadro.as_ref().map(|d| d.fbo),
            };
            unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, fbo) };
            return;
        }
        let medida = self.estado.frame_size();
        let (fw, fh) = (medida.0 as i32, medida.1 as i32);
        let n = self.escala as i32;
        let gl = &self.gl;
        unsafe {
            if self.reduzido.as_ref().is_none_or(|r| r.2 != medida) {
                if let Some((fbo, cor, _)) = self.reduzido.take() {
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(cor);
                }
                let cor = gl.create_texture().expect("textura reduzida");
                gl.bind_texture(glow::TEXTURE_2D, Some(cor));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    fw,
                    fh,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                let fbo = gl.create_framebuffer().expect("framebuffer reduzido");
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(cor),
                    0,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
                self.reduzido = Some((fbo, cor, medida));
            }
            let origem = self.quadro.as_ref().map(|d| d.fbo);
            let destino = self.reduzido.as_ref().map(|r| r.0);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, origem);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, destino);
            gl.disable(glow::SCISSOR_TEST);
            self.esquece_a_tesoura();
            // Na proporção larga, o jogo lê só o centro: é ali que está a imagem de 640×480 que
            // ele desenhou, e os lados são nossos.
            gl.blit_framebuffer(
                extra * n,
                0,
                (extra + fw) * n,
                fh * n,
                0,
                0,
                fw,
                fh,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, destino);
        }
    }

    /// Lê o quadro para `self.pixels` em RGB565, dois bytes por pixel na ordem do host, com a
    /// linha 0 no topo. Devolve `false` quando leu em RGBA: no GLES a leitura em 5-6-5 não é
    /// garantida, e ali fica o formato que sempre vale.
    fn le_quadro_rgb565(&mut self, largura: usize, altura: usize) -> bool {
        if self.gl.version().is_embedded {
            self.le_quadro(largura, altura);
            return false;
        }
        self.liga_para_leitura();
        self.pixels.clear();
        self.pixels.resize(largura * altura * 2, 0);
        unsafe {
            // Cada linha tem `largura * 2` bytes; o alinhamento padrão de quatro enviesaria
            // larguras ímpares.
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 2);
            self.gl.read_pixels(
                0,
                0,
                largura as i32,
                altura as i32,
                glow::RGB,
                glow::UNSIGNED_SHORT_5_6_5,
                glow::PixelPackData::Slice(Some(&mut self.pixels)),
            );
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
        }
        self.devolve_o_contexto();
        true
    }

    /// Lê o quadro da placa para `self.pixels`, em RGBA, com a linha 0 no topo.
    fn le_quadro(&mut self, largura: usize, altura: usize) {
        self.liga_para_leitura();
        self.pixels.clear();
        self.pixels.resize(largura * altura * 4, 0);
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                largura as i32,
                altura as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut self.pixels)),
            );
        }
        self.devolve_o_contexto();
    }

    /// Devolve o estado que o egui pressupõe, quando o contexto é de outro.
    ///
    /// No caminho com janela o egui pinta na **mesma thread e no mesmo contexto**, logo depois de
    /// nós, e ele não reconfigura tudo o que usa: deixar o nosso framebuffer ligado, ou o teste
    /// de profundidade aceso com uma profundidade que não é a dele, faz a interface desaparecer.
    ///
    /// Com contexto próprio isto não custa nada porque não roda: ninguém mais o usa.
    ///
    /// **Aqui, e não só no contexto refeito, morre o cache dos objetos ligados** quando quem
    /// apresenta é o anfitrião. Ver o fim desta função.
    fn devolve_o_contexto(&self) {
        if !self.placa.de_outro() {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::STENCIL_TEST);
            if !gl.version().is_embedded {
                gl.disable(glow::DEPTH_CLAMP);
            }
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
            gl.depth_range_f32(0.0, 1.0);
            gl.stencil_mask(u32::MAX);
            gl.color_mask(true, true, true, true);
        }
        // **Aqui o contexto vai para outra pessoa — e o espelho aprende o que ficou.**
        //
        // Esquecer tudo seria mais seguro e não pouparia nada: esta função roda a cada lote, e
        // um espelho zerado a cada lote responde "mudou" dezoito vezes sempre. Medido: zero
        // chamadas poupadas. O que se faz é registrar o estado **conhecido** que ela deixa, que é
        // o que o próximo `aplica` vai comparar — e as chaves que não estão aqui continuam
        // desconhecidas, porque o outro pode ter mexido nelas.
        let mut m = self.espelho.get();
        m.tesoura = None;
        m.tesoura_ligada = Some(false);
        m.teste_de_profundidade = Some(false);
        m.mistura = Some(false);
        m.descarte = Some(false);
        m.teste_de_estencil = Some(false);
        m.mascara_profundidade = Some(true);
        m.faixa_profundidade = Some((0.0, 1.0));
        m.mascara_estencil = Some(u32::MAX);
        m.mascara_cor = Some([true, true, true, true]);
        if !self.gl.version().is_embedded {
            m.abraco_de_profundidade = Some(false);
        }
        self.espelho.set(m);
        // **E o cache dos três objetos ligados morre aqui também**, quando quem apresenta o quadro
        // é o anfitrião: sem um framebuffer de fora, quem desenha o nosso destino é a interface, no
        // mesmo contexto, logo depois de nós — e o `Pintor` dela termina cada pintura **desligando**
        // programa e VAO de propósito (`src/ui/gpu.rs`). Confiar no cache depois disso é desenhar
        // com o programa do `egui`: no perfil de núcleo, VAO zero é `GL_INVALID_OPERATION` e nada
        // na tela, e sem erro nenhum do lado de cá.
        //
        // **Quando o frontend entrega o framebuffer dele, não**: ali quem desenha é o libretro, o
        // quadro é dele, e o `devolve_o_contexto` roda a cada lote — invalidar aqui seria pagar três
        // chamadas de driver por lote de novo, que é justamente o custo que o `e6a436e` tirou do
        // caminho do RetroArch (370 lotes por quadro no Quake). Nesse caminho quem invalida é o
        // [`GpuState::desenha_no_fbo`], uma vez por quadro, quando o frontend pega o contexto.
        if self.fbo_externo.is_none() {
            self.placa.esquece_o_ligado();
        }
    }

    /// Reaplica os parâmetros de uma textura, rebaixando o filtro quando falta a cadeia.
    fn parametros(&self, t: &Textura) {
        let gl = &self.gl;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(t.objeto));
            let tem_cadeia = t.maior_nivel > 0;
            let min = match (t.filtro_min, tem_cadeia) {
                (f, true) => f,
                // Sem níveis auxiliares, um filtro de mipmap desenha preto no OpenGL. O
                // rasterizador de software cai para o nível zero nesse caso; aqui a queda é
                // escolher o filtro equivalente sem mipmap.
                (gles::GL_NEAREST_MIPMAP_NEAREST | gles::GL_NEAREST_MIPMAP_LINEAR, false) => {
                    gles::GL_NEAREST
                }
                (gles::GL_LINEAR_MIPMAP_NEAREST | gles::GL_LINEAR_MIPMAP_LINEAR, false) => {
                    gles::GL_LINEAR
                }
                (f, false) => f,
            };
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, t.filtro as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, t.maior_nivel as i32);
            for (eixo, modo) in [
                (glow::TEXTURE_WRAP_S, t.wrap[0]),
                (glow::TEXTURE_WRAP_T, t.wrap[1]),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, eixo, modo as i32);
            }
            // Só é pedido quando está ligado, e só chega aqui ligado se a placa tem a extensão —
            // ver [`Rasterizador::define_anisotropico`].
            if self.anisotropia > 1.0 {
                gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, self.anisotropia);
            }
        }
    }
}

impl Drop for GpuState {
    fn drop(&mut self) {
        // **Com a placa morta, nada é apagado.** O frontend avisa que o contexto acabou e a sessão
        // viva ainda guarda este estado: no `retro_run` seguinte ele é trocado, e o `Drop` corre
        // aqui. Chamar `delete_*` neste ponto seria usar os ponteiros de função de um contexto que
        // já não existe — no melhor caso o driver anota um erro, no pior o endereço aponta para
        // código que não está mais lá (auditoria de GPU, achado 1). Os nomes são largados com o
        // contexto, que é de quem avisou. Ver [`Placa::apaga_ao_morrer`].
        if !self.placa.apaga_ao_morrer() {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.delete_program(self.programa);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_texture(self.ponte);
            for t in self.texturas.values() {
                gl.delete_texture(t.objeto);
            }
            if let Some(d) = self.quadro.take() {
                solta_destino(gl, d);
            }
            if let Some((fbo, cor, _)) = self.reduzido.take() {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(cor);
            }
        }
    }
}

/// Devolve à placa o que um destino criou.
unsafe fn solta_destino(gl: &glow::Context, d: Destino) {
    unsafe {
        gl.delete_framebuffer(d.fbo);
        gl.delete_texture(d.cor);
        gl.delete_renderbuffer(d.profundidade);
        if let Some((fbo, cor)) = d.multi {
            gl.delete_framebuffer(fbo);
            gl.delete_renderbuffer(cor);
        }
    }
}

fn liga(gl: &glow::Context, capacidade: u32, ligado: bool) {
    unsafe {
        match ligado {
            true => gl.enable(capacidade),
            false => gl.disable(capacidade),
        }
    }
}

/// As posições dos uniformes do programa, e o último valor que cada um recebeu.
///
/// O Quake faz umas 370 draw calls por quadro, e cada uma procurava pelo nome a posição de uma
/// dezena de uniformes — o glow monta uma string C e o driver procura o nome a cada vez — e
/// reenviava todos, mesmo iguais. A posição não muda enquanto o programa existir, e um uniforme
/// guarda o valor no programa: mandar de novo o mesmo valor não muda nada. Medido, é perto de um
/// microssegundo a menos por desenho; o grosso do custo era o buffer, ver [`GpuState::anel`].
#[derive(Default)]
struct Uniformes {
    mapa: std::cell::RefCell<HashMap<String, (Option<glow::UniformLocation>, Option<[u32; 4]>)>>,
}

impl Uniformes {
    /// Manda `valor` para o uniforme `nome` por `envia`, se ele existir e o valor for novo.
    fn define(
        &self,
        gl: &glow::Context,
        programa: glow::Program,
        nome: &str,
        valor: [u32; 4],
        envia: impl FnOnce(&glow::UniformLocation),
    ) {
        let mut mapa = self.mapa.borrow_mut();
        if !mapa.contains_key(nome) {
            let onde = unsafe { gl.get_uniform_location(programa, nome) };
            mapa.insert(nome.to_string(), (onde, None));
        }
        let Some((onde, ultimo)) = mapa.get_mut(nome) else {
            return;
        };
        if *ultimo == Some(valor) {
            return;
        }
        *ultimo = Some(valor);
        if let Some(onde) = onde {
            envia(onde);
        }
    }
}

fn uniforme_i32(gl: &glow::Context, u: &Uniformes, programa: glow::Program, nome: &str, valor: i32) {
    u.define(gl, programa, nome, [valor as u32, 0, 0, 0], |onde| unsafe {
        gl.uniform_1_i32(Some(onde), valor)
    });
}

fn uniforme_f32(gl: &glow::Context, u: &Uniformes, programa: glow::Program, nome: &str, valor: f32) {
    u.define(gl, programa, nome, [valor.to_bits(), 0, 0, 0], |onde| unsafe {
        gl.uniform_1_f32(Some(onde), valor)
    });
}

/// Um uniforme de quatro componentes, para a cor do `GL_TEXTURE_ENV_COLOR`.
fn uniforme_vec4(
    gl: &glow::Context,
    u: &Uniformes,
    programa: glow::Program,
    nome: &str,
    valor: [f32; 4],
) {
    u.define(gl, programa, nome, valor.map(f32::to_bits), |onde| unsafe {
        gl.uniform_4_f32(Some(onde), valor[0], valor[1], valor[2], valor[3])
    });
}

/// Um uniforme de três componentes, para a cor da névoa.
fn uniforme_vec3(
    gl: &glow::Context,
    u: &Uniformes,
    programa: glow::Program,
    nome: &str,
    valor: [f32; 4],
) {
    let bits = [valor[0].to_bits(), valor[1].to_bits(), valor[2].to_bits(), 0];
    u.define(gl, programa, nome, bits, |onde| unsafe {
        gl.uniform_3_f32(Some(onde), valor[0], valor[1], valor[2])
    });
}

/// Escreve um vértice no buffer da placa, no layout que [`FLOATS_POR_VERTICE`] declara.
///
/// **É o único lugar que conhece esse layout.** O fator da névoa vem pronto da etapa de
/// vértice, que é a mesma dos dois rasterizadores; aqui ele é só mais um atributo a interpolar.
fn poe_em(destino: &mut Vec<f32>, v: &Vertex) {
    destino.extend_from_slice(&v.position);
    destino.extend_from_slice(&v.color);
    destino.extend_from_slice(&v.uv);
    destino.push(v.fog);
    destino.extend_from_slice(&v.uv1);
}

/// Os bytes de um vetor de `f32`, para o `buffer_data`.
fn bytes_de_f32(dados: &[f32]) -> &[u8] {
    // Um f32 nao tem invariante de bits, e o alinhamento de quatro serve para um de um.
    unsafe { std::slice::from_raw_parts(dados.as_ptr().cast(), std::mem::size_of_val(dados)) }
}

/// Os texels RGBA ja estao no formato de quatro bytes que a placa recebe.
fn bytes_de_rgba(dados: &[[u8; 4]]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(dados.as_ptr().cast(), dados.len() * 4) }
}

/// O código que o shader usa para cada modo de `glTexEnv`. Ver [`TexEnv::aplica`].
fn codigo_env(modo: u32) -> i32 {
    match modo {
        gles::GL_REPLACE => 0,
        gles::GL_DECAL => 1,
        gles::GL_ADD => 2,
        gles::GL_COMBINE => 4,
        _ => 3,
    }
}

/// A função do `GL_COMBINE` no shader. Ver `TexEnv::combina`.
fn codigo_funcao(funcao: u32) -> i32 {
    match funcao {
        gles::GL_REPLACE => 0,
        gles::GL_ADD => 2,
        gles::GL_ADD_SIGNED => 3,
        gles::GL_INTERPOLATE => 4,
        gles::GL_SUBTRACT => 5,
        gles::GL_DOT3_RGB => 6,
        gles::GL_DOT3_RGBA => 7,
        _ => 1,
    }
}

/// A fonte do `GL_COMBINE` no shader: textura, constante, cor primária ou unidade anterior.
fn codigo_fonte(fonte: u32) -> i32 {
    match fonte {
        gles::GL_TEXTURE => 0,
        gles::GL_CONSTANT => 1,
        gles::GL_PRIMARY_COLOR => 2,
        _ => 3,
    }
}

/// Os uniformes do ambiente de uma unidade; `sufixo` é `""` para a 0 e `"1"` para a 1.
fn envia_env(
    gl: &glow::Context,
    u: &Uniformes,
    programa: glow::Program,
    sufixo: &str,
    env: &TexEnv,
) {
    // O nome do modo sai pronto: ele é mandado a cada desenho, e o resto só no `GL_COMBINE`.
    let modo = if sufixo.is_empty() { "env" } else { "env1" };
    uniforme_i32(gl, u, programa, modo, codigo_env(env.modo));
    if env.modo != gles::GL_COMBINE {
        return;
    }
    for lado in 0..2 {
        let nome = ["rgb", "alfa"][lado];
        uniforme_i32(gl, u, programa, &format!("cmb_{nome}{sufixo}"), codigo_funcao(env.combina[lado]));
        uniforme_f32(gl, u, programa, &format!("escala_{nome}{sufixo}"), env.escala[lado]);
        for i in 0..3 {
            uniforme_i32(gl, u, programa, &format!("src_{nome}{sufixo}[{i}]"), codigo_fonte(env.fontes[lado][i]));
            uniforme_i32(gl, u, programa, &format!("op_{nome}{sufixo}[{i}]"), codigo_operando(env.operandos[lado][i]));
        }
    }
    uniforme_vec4(gl, u, programa, &format!("cor_env{sufixo}"), env.cor);
}

/// O operando do `GL_COMBINE` no shader.
fn codigo_operando(operando: u32) -> i32 {
    match operando {
        gles::GL_SRC_COLOR => 0,
        gles::GL_ONE_MINUS_SRC_COLOR => 1,
        gles::GL_SRC_ALPHA => 2,
        _ => 3,
    }
}

/// O código que o shader usa para cada função de teste de alfa.
fn codigo_alfa(func: u32) -> i32 {
    match func {
        gles::GL_NEVER => 0,
        gles::GL_LESS => 1,
        gles::GL_EQUAL => 2,
        gles::GL_LEQUAL => 3,
        gles::GL_GREATER => 4,
        gles::GL_NOTEQUAL => 5,
        gles::GL_GEQUAL => 6,
        _ => 7,
    }
}

/// O shader de vértice é **passagem direta**: as posições já chegam em espaço de recorte, porque
/// a etapa de vértice acontece na CPU, compartilhada com o rasterizador de software.
///
/// O `virar` inverte o Y. Ele existe porque a linha 0 do framebuffer é tratada como o topo da
/// imagem — assim o `glReadPixels` devolve as linhas na mesma ordem que a superfície de software
/// usa, e nenhum quadro precisa ser espelhado na CPU. A ponte do `import` passa `+1` porque já
/// entrega coordenadas prontas.
///
/// Não há caminho de sombreamento plano porque o rasterizador de software não tem: ele guarda o
/// `glShadeModel` e nunca o lê. Ver [`GpuState::set_shade_model`].
const VERTICE: &str = r#"
layout(location = 0) in vec4 pos;
layout(location = 1) in vec4 cor;
layout(location = 2) in vec2 uv;
layout(location = 3) in float fog;
layout(location = 4) in vec2 uv1;
uniform float virar;
out vec4 vcor;
out vec2 vuv;
out float vfog;
out vec2 vuv1;
void main() {
    vcor = cor;
    vuv = uv;
    vfog = fog;
    vuv1 = uv1;
    gl_Position = vec4(pos.x, pos.y * virar, pos.z, pos.w);
}
"#;

/// O fragmento faz o que sobrou do pipeline fixo: combinar a textura e testar o alfa.
///
/// A combinação segue `combine` do rasterizador de software, caso a caso, para que os dois
/// backends respondam a `glTexEnv` do mesmo jeito.
const FRAGMENTO: &str = r#"
in vec4 vcor;
in vec2 vuv;
in float vfog;
in vec2 vuv1;
uniform sampler2D amostra;
uniform sampler2D amostra1;
uniform int com_neblina;
uniform vec3 cor_neblina;
uniform int texturando;
uniform int texturando1;
// O ambiente de cada unidade: o modo e, no `GL_COMBINE`, função, fontes, operandos, escalas e a
// cor constante. O sufixo `1` é a unidade 1.
uniform int env;
uniform int cmb_rgb;
uniform int cmb_alfa;
uniform int src_rgb[3];
uniform int op_rgb[3];
uniform int src_alfa[3];
uniform int op_alfa[3];
uniform float escala_rgb;
uniform float escala_alfa;
uniform vec4 cor_env;
uniform int env1;
uniform int cmb_rgb1;
uniform int cmb_alfa1;
uniform int src_rgb1[3];
uniform int op_rgb1[3];
uniform int src_alfa1[3];
uniform int op_alfa1[3];
uniform float escala_rgb1;
uniform float escala_alfa1;
uniform vec4 cor_env1;
uniform int func_alfa;
uniform float ref_alfa;
out vec4 saida;

// A fonte do `GL_COMBINE`: 0 a textura, 1 a constante, 2 a cor primária, 3 a unidade anterior.
vec4 fonte_de(int qual, vec4 anterior, vec4 primaria, vec4 texel, vec4 constante) {
    if (qual == 0) { return texel; }
    if (qual == 1) { return constante; }
    if (qual == 2) { return primaria; }
    return anterior;
}

// O operando no RGB: a cor, o complemento dela, o alfa ou o complemento dele.
vec3 operando_rgb(int op, vec4 v) {
    if (op == 0) { return v.rgb; }
    if (op == 1) { return vec3(1.0) - v.rgb; }
    if (op == 2) { return vec3(v.a); }
    return vec3(1.0 - v.a);
}

// No alfa só há os operandos de alfa; os de cor valem como eles, como no software.
float operando_alfa(int op, vec4 v) {
    if (op == 1 || op == 3) { return 1.0 - v.a; }
    return v.a;
}

vec3 funcao_rgb(int f, vec3 a0, vec3 a1, vec3 a2) {
    if (f == 0) { return a0; }
    if (f == 2) { return a0 + a1; }
    if (f == 3) { return a0 + a1 - 0.5; }
    if (f == 4) { return a0 * a2 + a1 * (1.0 - a2); }
    if (f == 5) { return a0 - a1; }
    if (f == 6 || f == 7) { return vec3(4.0 * dot(a0 - 0.5, a1 - 0.5)); }
    return a0 * a1;
}

float funcao_alfa(int f, float a0, float a1, float a2) {
    if (f == 0) { return a0; }
    if (f == 2) { return a0 + a1; }
    if (f == 3) { return a0 + a1 - 0.5; }
    if (f == 4) { return a0 * a2 + a1 * (1.0 - a2); }
    if (f == 5) { return a0 - a1; }
    return a0 * a1;
}

// Uma unidade de textura, caso a caso igual ao `TexEnv::aplica_com` do software: os modos
// clássicos agem sobre o que saiu da unidade anterior.
vec4 unidade(int modo, int crgb, int calfa, int srgb[3], int orgb[3], int salfa[3], int oalfa[3],
             float ergb, float ealfa, vec4 constante, vec4 anterior, vec4 primaria, vec4 texel) {
    if (modo == 4) {
        vec3 r[3];
        float a[3];
        for (int i = 0; i < 3; i++) {
            r[i] = operando_rgb(orgb[i], fonte_de(srgb[i], anterior, primaria, texel, constante));
            a[i] = operando_alfa(oalfa[i], fonte_de(salfa[i], anterior, primaria, texel, constante));
        }
        vec3 rgb = clamp(funcao_rgb(crgb, r[0], r[1], r[2]) * ergb, 0.0, 1.0);
        float alfa = clamp(funcao_alfa(calfa, a[0], a[1], a[2]) * ealfa, 0.0, 1.0);
        if (crgb == 7) { alfa = rgb.r; }
        return vec4(rgb, alfa);
    }
    if (modo == 0) { return texel; }
    if (modo == 1) {
        return vec4(anterior.rgb * (1.0 - texel.a) + texel.rgb * texel.a, anterior.a);
    }
    if (modo == 2) {
        return vec4(min(anterior.rgb + texel.rgb, vec3(1.0)), anterior.a * texel.a);
    }
    return anterior * texel;
}

void main() {
    vec4 primaria = vcor;
    vec4 cor = primaria;
    if (texturando == 1) {
        cor = unidade(env, cmb_rgb, cmb_alfa, src_rgb, op_rgb, src_alfa, op_alfa, escala_rgb,
                      escala_alfa, cor_env, cor, primaria, texture(amostra, vuv));
    }
    if (texturando1 == 1) {
        cor = unidade(env1, cmb_rgb1, cmb_alfa1, src_rgb1, op_rgb1, src_alfa1, op_alfa1,
                      escala_rgb1, escala_alfa1, cor_env1, cor, primaria, texture(amostra1, vuv1));
    }
    // A névoa entra depois da textura e antes do teste de alfa, e mexe só no RGB — a mesma
    // ordem do rasterizador de software.
    if (com_neblina == 1) {
        cor = vec4(mix(cor_neblina, cor.rgb, clamp(vfog, 0.0, 1.0)), cor.a);
    }
    bool passa;
    if      (func_alfa == 0) { passa = false; }
    else if (func_alfa == 1) { passa = cor.a <  ref_alfa; }
    else if (func_alfa == 2) { passa = cor.a == ref_alfa; }
    else if (func_alfa == 3) { passa = cor.a <= ref_alfa; }
    else if (func_alfa == 4) { passa = cor.a >  ref_alfa; }
    else if (func_alfa == 5) { passa = cor.a != ref_alfa; }
    else if (func_alfa == 6) { passa = cor.a >= ref_alfa; }
    else                     { passa = true; }
    if (!passa) { discard; }
    saida = cor;
}
"#;

/// Compila o par de shaders, tentando GLSL 3.30 e caindo para ES 3.00.
unsafe fn compila(gl: &glow::Context) -> Result<glow::Program, String> {
    ["#version 330 core\n", "#version 300 es\nprecision highp float;\n"]
        .into_iter()
        .find_map(|cabecalho| unsafe { liga_programa(gl, cabecalho) }.ok())
        .ok_or_else(|| "nenhuma versão de GLSL aceita".to_string())
}

unsafe fn liga_programa(gl: &glow::Context, cabecalho: &str) -> Result<glow::Program, String> {
    unsafe {
        let programa = gl.create_program()?;
        let mut shaders = Vec::new();
        for (tipo, fonte) in [
            (glow::VERTEX_SHADER, VERTICE),
            (glow::FRAGMENT_SHADER, FRAGMENTO),
        ] {
            let shader = gl.create_shader(tipo)?;
            gl.shader_source(shader, &format!("{cabecalho}{fonte}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let erro = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for shader in shaders {
                    gl.delete_shader(shader);
                }
                gl.delete_program(programa);
                return Err(erro);
            }
            gl.attach_shader(programa, shader);
            shaders.push(shader);
        }
        gl.link_program(programa);
        for shader in shaders {
            gl.detach_shader(programa, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(programa) {
            let erro = gl.get_program_info_log(programa);
            gl.delete_program(programa);
            return Err(erro);
        }
        Ok(programa)
    }
}

/// Um pixel RGB565 expandido para RGB888. Mesma expansão do rasterizador de software.
fn expande565(bytes: &[u8], offset: usize) -> [u8; 3] {
    let pixel = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let r = ((pixel >> 11) & 31) as u8;
    let g = ((pixel >> 5) & 63) as u8;
    let b = (pixel & 31) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

impl Rasterizador for GpuState {
    fn grava_estado(&self, destino: &mut crate::save_state::Secoes) {
        // O estado é o mesmo `GlState` que o rasterizador de software usa — este apenas o desenha
        // na placa em vez de no processador. Gravar é delegar.
        crate::save_state::Guardavel::grava(&self.estado, destino);
    }

    fn restaura_estado(
        &mut self,
        origem: &crate::save_state::Leitor<'_>,
    ) -> Result<(), crate::save_state::Erro> {
        crate::save_state::Guardavel::restaura(&mut self.estado, origem)?;
        self.descarta_caches_de_quadro();
        self.recria_texturas_restauradas();
        // O espelho de estado de desenho é mantido **incrementalmente** pelos métodos do trait, e
        // não reconstruído do GlState. Depois de carregar, ele descreveria o mundo anterior, e é
        // ele que vai para o GL a cada lote — a cena sairia com a matriz e as bandeiras de antes.
        self.ressincroniza_o_espelho();
        self.devolve_o_contexto();
        Ok(())
    }

    fn descarrega_o_desenho(&mut self) {
        self.descarrega();
    }

    fn desenho_em_curso(&self) -> bool {
        self.estado.desenho_em_curso()
    }

    /// O framebuffer de fora, quando o frontend entrega um — e com ele a **fronteira do quadro**.
    ///
    /// **É aqui que o cache dos objetos ligados é invalidado no caminho do libretro.** Entre dois
    /// quadros o frontend desenha o FBO dele no mesmo contexto, ligando o que precisa — e o
    /// `e6a436e` mediu que, naquele consumidor, o que ele deixa é inofensivo para nós. O que não se
    /// pode é *supor* isso: esta chamada é a única que o core faz uma vez por quadro, antes de
    /// qualquer desenho, e é a hora certa de não confiar no que ficou de antes. Custa três chamadas
    /// de driver por quadro, e não por lote, mais o estado de desenho reenviado uma vez no primeiro
    /// lote — o espelho também esquece, pelo mesmo motivo.
    fn desenha_no_fbo(&mut self, fbo: Option<u32>) {
        if fbo.is_some() {
            self.esquece_o_espelho();
            self.placa.esquece_o_ligado();
        }
        self.fbo_externo = fbo;
    }

    fn set_matrix_mode(&mut self, mode: u32) {
        self.estado.set_matrix_mode(mode);
    }
    fn load_identity(&mut self) {
        self.estado.load_identity();
    }
    fn load_matrix(&mut self, m: Matrix) {
        self.estado.load_matrix(m);
    }
    fn mult_matrix(&mut self, m: Matrix) {
        self.estado.mult_matrix(m);
    }
    fn push_matrix(&mut self) {
        self.estado.push_matrix();
    }
    fn pop_matrix(&mut self) {
        self.estado.pop_matrix();
    }

    fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.estado.set_viewport(x, y, width, height);
        self.fill.viewport = (x, y, width, height);
    }
    fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.estado.set_scissor(x, y, width, height);
        self.fill.tesoura = (x, y, width, height);
    }
    fn set_surface(&mut self, width: usize, height: usize) {
        self.descarrega();
        self.estado.set_surface(width, height);
    }
    fn set_surface_esticada(&mut self, width: usize, height: usize) {
        self.descarrega();
        self.estado.set_surface_esticada(width, height);
    }
    fn surface(&self) -> (usize, usize) {
        self.estado.surface()
    }
    fn frame_size(&self) -> (usize, usize) {
        self.estado.frame_size()
    }

    fn set_clear_color(&mut self, color: [f32; 4]) {
        self.estado.set_clear_color(color);
        self.fill.limpa_cor = color;
    }
    fn set_clear_depth(&mut self, depth: f32) {
        self.estado.set_clear_depth(depth);
        self.fill.limpa_profundidade = depth;
    }
    fn set_clear_stencil(&mut self, valor: i32) {
        self.estado.set_clear_stencil(valor);
        self.fill.limpa_stencil = valor;
    }
    fn set_color(&mut self, color: [f32; 4]) {
        self.estado.set_color(color);
    }
    fn current_color(&self) -> [f32; 4] {
        self.estado.current_color()
    }

    fn clear(&mut self, mask: u32) {
        self.descarrega();
        self.destino();
        // **O `clear` do rasterizador de software ignora as máscaras**: ele preenche os vetores
        // direto. O `glClear` respeita `glDepthMask`, `glStencilMask` e `glColorMask`, então elas
        // são abertas aqui — senão uma limpeza pedida com máscara fechada simplesmente não
        // aconteceria, e o quadro seguinte desenharia sobre o anterior.
        let gl = &self.gl;
        let mut bits = 0;
        unsafe {
            if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
                let [r, g, b, a] = self.fill.limpa_cor;
                gl.color_mask(true, true, true, true);
                gl.clear_color(r, g, b, a);
                bits |= glow::COLOR_BUFFER_BIT;
            }
            if mask & gles::GL_DEPTH_BUFFER_BIT != 0 {
                gl.depth_mask(true);
                gl.clear_depth_f32(self.fill.limpa_profundidade);
                bits |= glow::DEPTH_BUFFER_BIT;
            }
            if mask & gles::GL_STENCIL_BUFFER_BIT != 0 {
                gl.stencil_mask(u32::MAX);
                gl.clear_stencil(self.fill.limpa_stencil);
                bits |= glow::STENCIL_BUFFER_BIT;
            }
            if bits != 0 {
                // O recorte do `glScissor` fica de fora: o rasterizador de software não o tem, e
                // divergir aqui tornaria a comparação entre os dois inútil.
                gl.disable(glow::SCISSOR_TEST);
                self.esquece_a_tesoura();
                gl.clear(bits);
            }
        }
        self.devolve_o_contexto();
        if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
            self.sujo = true;
        }
    }

    fn set_capability(&mut self, capability: u32, on: bool) {
        self.estado.set_capability(capability, on);
        match capability {
            gles::GL_DEPTH_TEST => self.fill.teste_profundidade = on,
            gles::GL_BLEND => self.fill.mistura = on,
            gles::GL_ALPHA_TEST => self.fill.teste_alfa = on,
            gles::GL_CULL_FACE => self.fill.descarte = on,
            gles::GL_STENCIL_TEST => self.fill.teste_stencil = on,
            gles::GL_SCISSOR_TEST => {
                self.fill.tesoura_ligada = on;
                self.estado.set_scissor_test(on);
            }
            // Ligar e desligar textura é por unidade; as duas primeiras desenham.
            gles::GL_TEXTURE_2D if self.estado.base_active_unit() => self.fill.texturando = on,
            gles::GL_TEXTURE_2D => self.fill.unidade1 = self.estado.unidade1(),
            gles::GL_FOG => self.fill.neblina = self.estado.neblina(),
            _ => {}
        }
    }
    fn set_shade_model(&mut self, mode: u32) {
        // **Os dois backends ignoram o `glShadeModel`**, e de propósito: o rasterizador de
        // software guarda o modo e nunca o lê — sempre interpola. É uma divergência do GLES1 que
        // eles compartilham, e fazer a placa honrar o modo plano aqui só tornaria a comparação
        // entre os dois ruidosa sem corrigir nada que se veja.
        self.estado.set_shade_model(mode);
    }
    fn set_light(&mut self, index: usize, pname: u32, valores: [f32; 4]) {
        self.estado.set_light(index, pname, valores);
    }
    fn set_material(&mut self, pname: u32, valores: [f32; 4]) {
        self.estado.set_material(pname, valores);
    }
    fn set_light_model(&mut self, pname: u32, valores: [f32; 4]) {
        self.estado.set_light_model(pname, valores);
    }

    fn set_blend_func(&mut self, src: u32, dst: u32) {
        self.estado.set_blend_func(src, dst);
        self.fill.mistura_src = src;
        self.fill.mistura_dst = dst;
    }
    fn set_alpha_func(&mut self, func: u32, reference: f32) {
        self.estado.set_alpha_func(func, reference);
        self.fill.func_alfa = func;
        self.fill.ref_alfa = reference;
    }
    fn set_depth_func(&mut self, func: u32) {
        self.estado.set_depth_func(func);
        self.fill.func_profundidade = func;
    }
    fn set_depth_mask(&mut self, on: bool) {
        self.estado.set_depth_mask(on);
        self.fill.mascara_profundidade = on;
    }
    fn set_depth_range(&mut self, perto: f32, longe: f32) {
        self.estado.set_depth_range(perto, longe);
        self.fill.faixa_profundidade = (perto.clamp(0.0, 1.0), longe.clamp(0.0, 1.0));
    }
    fn set_fog(&mut self, pname: u32, valores: [f32; 4]) {
        self.estado.set_fog(pname, valores);
        self.fill.neblina = self.estado.neblina();
    }
    fn define_neblina(&mut self, permitida: bool) {
        self.estado.define_neblina(permitida);
        self.fill.neblina = self.estado.neblina();
    }
    fn set_color_mask(&mut self, mask: [bool; 4]) {
        self.estado.set_color_mask(mask);
        self.fill.mascara_cor = mask;
    }
    fn set_cull_face(&mut self, mode: u32) {
        self.estado.set_cull_face(mode);
        self.fill.modo_descarte = mode;
    }
    fn set_front_face(&mut self, face: u32) {
        self.estado.set_front_face(face);
        self.fill.face_frontal = face;
    }
    fn set_stencil_func(&mut self, func: u32, referencia: i32, mask: u32) {
        self.estado.set_stencil_func(func, referencia, mask);
        self.fill.func_stencil = func;
        self.fill.ref_stencil = referencia;
        self.fill.mascara_valor_stencil = mask;
    }
    fn set_stencil_op(&mut self, falha: u32, falha_z: u32, passa: u32) {
        self.estado.set_stencil_op(falha, falha_z, passa);
        self.fill.op_stencil = [falha, falha_z, passa];
    }
    fn set_stencil_mask(&mut self, mask: u32) {
        self.estado.set_stencil_mask(mask);
        self.fill.mascara_escrita_stencil = mask;
    }

    fn set_active_texture(&mut self, unit: u32) {
        self.estado.set_active_texture(unit);
    }
    fn set_client_active_texture(&mut self, unit: u32) {
        self.estado.set_client_active_texture(unit);
    }
    fn client_unit(&self) -> u32 {
        Rasterizador::client_unit(&self.estado)
    }
    // **A ligação é da unidade ativa**, aqui como no estado de software: o Resident Evil 4
    // termina cada bloco ligando textura na unidade 1, e deixar essa ligação valer na base
    // trocava a textura dela e pintava a vila de branco. Ver o `active_unit` do `GlState`.
    fn bind_texture(&mut self, name: u32) {
        self.estado.bind_texture(name);
        if self.estado.base_active_unit() {
            self.fill.textura_ligada = name;
        }
        self.fill.unidade1 = self.estado.unidade1();
    }
    fn bound_texture(&self) -> u32 {
        self.estado.bound_texture()
    }
    fn set_texture_env(&mut self, mode: u32) {
        self.estado.set_texture_env(mode);
        self.fill.env_textura = self.estado.texture_env();
        self.fill.unidade1 = self.estado.unidade1();
    }
    fn set_texture_env_param(&mut self, pname: u32, enumeracao: u32, numero: f32) {
        self.estado.set_texture_env_param(pname, enumeracao, numero);
        self.fill.env_textura = self.estado.texture_env();
        self.fill.unidade1 = self.estado.unidade1();
    }
    fn set_texture_env_color(&mut self, cor: [f32; 4]) {
        self.estado.set_texture_env_color(cor);
        self.fill.env_textura = self.estado.texture_env();
        self.fill.unidade1 = self.estado.unidade1();
    }
    fn set_texture_parameter(&mut self, name: u32, value: u32) {
        self.descarrega();
        self.estado.set_texture_parameter(name, value);
        if !self.estado.unidade_ativa_desenha() {
            return;
        }
        let ligada = self.estado.bound_texture();
        if let Some(t) = self.texturas.get_mut(&ligada) {
            match name {
                gles::GL_TEXTURE_MIN_FILTER => t.filtro_min = value,
                gles::GL_TEXTURE_MAG_FILTER => t.filtro = value,
                gles::GL_TEXTURE_WRAP_S => t.wrap[0] = value,
                gles::GL_TEXTURE_WRAP_T => t.wrap[1] = value,
                _ => {}
            }
        }
        if let Some(t) = self.texturas.get(&ligada) {
            self.parametros(t);
        }
    }
    fn set_texture_crop(&mut self, crop: [i32; 4]) {
        self.estado.set_texture_crop(crop);
        if !self.estado.unidade_ativa_desenha() {
            return;
        }
        let ligada = self.estado.bound_texture();
        if let Some(t) = self.texturas.get_mut(&ligada) {
            t.crop = crop;
        }
    }
    fn delete_texture(&mut self, name: u32) {
        self.descarrega();
        self.estado.delete_texture(name);
        if let Some(t) = self.texturas.remove(&name) {
            // Com a placa morta o nome é largado, e não apagado: ver [`Placa::apaga_ao_morrer`].
            if self.placa.apaga_ao_morrer() {
                unsafe { self.gl.delete_texture(t.objeto) };
            }
        }
    }

    fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    ) {
        self.descarrega();
        let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
        let gl = &self.gl;
        let objeto = match self.texturas.get(&name) {
            Some(t) => t.objeto,
            None => {
                let objeto = match unsafe { gl.create_texture() } {
                    Ok(objeto) => objeto,
                    Err(_) => return,
                };
                self.texturas.insert(
                    name,
                    Textura {
                        objeto,
                        largura: width,
                        altura: height,
                        maior_nivel: 0,
                        crop: [0; 4],
                        filtro: gles::GL_LINEAR,
                        filtro_min: gles::GL_LINEAR,
                        wrap: [gles::GL_REPEAT; 2],
                    },
                );
                objeto
            }
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(objeto));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                level as i32,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&bytes)),
            );
        }
        if let Some(t) = self.texturas.get_mut(&name) {
            if level == 0 {
                t.largura = width;
                t.altura = height;
            }
            t.maior_nivel = t.maior_nivel.max(level);
        }
        if let Some(t) = self.texturas.get(&name) {
            self.parametros(t);
        }
        self.estado.upload_level(name, level, width, height, pixels);
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
        self.descarrega();
        let resultado = self.estado.sub_image(name, x, y, width, height, pixels);
        if resultado.is_ok() {
            if let Some(t) = self.texturas.get(&name) {
                let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
                unsafe {
                    let gl = &self.gl;
                    gl.bind_texture(glow::TEXTURE_2D, Some(t.objeto));
                    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        x as i32,
                        y as i32,
                        width as i32,
                        height as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&bytes)),
                    );
                }
            }
        }
        resultado
    }

    fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
        // Pontos e linhas não aparecem nos jogos do console, e o rasterizador de software também
        // os deixa sem tratamento. Desenhá-los aqui divergiria dele sem ganho nenhum.
        let modo = match mode {
            gles::GL_TRIANGLES => glow::TRIANGLES,
            gles::GL_TRIANGLE_STRIP => glow::TRIANGLE_STRIP,
            gles::GL_TRIANGLE_FAN => glow::TRIANGLE_FAN,
            _ => return,
        };
        self.estado.etapa_de_vertice(vertices);
        let extra = self.extra();
        let perspectiva = extra > 0 && self.estado.projecao_em_perspectiva();
        // O `x` de recorte encolhe na razão em que a viewport cresce — ver [`GpuState::aplica`].
        let k = match perspectiva {
            true => {
                let sw = self.estado.surface().0 as f32;
                sw / (sw + 2.0 * extra as f32)
            }
            false => 1.0,
        };
        // Estado diferente do lote em curso: o que já foi juntado vai antes, com o estado dele.
        if self
            .estado_do_lote
            .as_ref()
            .is_some_and(|(estado, p)| *p != perspectiva || *estado != self.fill)
            || self.lote.len() >= VERTICES_NO_LOTE * FLOATS_POR_VERTICE
        {
            self.descarrega();
        }
        if self.estado_do_lote.is_none() {
            self.estado_do_lote = Some((self.fill.clone(), perspectiva));
        }
        let mut soltos = std::mem::take(&mut self.soltos);
        soltos.clear();
        soltos.extend(self.estado.transformados().iter().map(|v| {
            let [px, py, pz, pw] = v.position;
            Vertex {
                position: [px * k, py, pz, pw],
                ..*v
            }
        }));
        // Leque e faixa viram triângulos soltos na ordem em que o OpenGL os monta, que é o que
        // mantém a orientação — e com ela o descarte por face. Na faixa, a cada passo a
        // orientação alterna, e trocar os dois primeiros a mantém.
        let n = soltos.len();
        let mut poe = |i: usize| poe_em(&mut self.lote, &soltos[i]);
        match modo {
            glow::TRIANGLES => (0..n / 3 * 3).for_each(&mut poe),
            glow::TRIANGLE_STRIP => {
                for i in 0..n.saturating_sub(2) {
                    let (a, b) = if i % 2 == 0 { (i, i + 1) } else { (i + 1, i) };
                    poe(a);
                    poe(b);
                    poe(i + 2);
                }
            }
            _ => {
                for i in 1..n.saturating_sub(1) {
                    poe(0);
                    poe(i);
                    poe(i + 1);
                }
            }
        }
        self.soltos = soltos;
    }

    fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32) {
        self.descarrega();
        let ligada = self.fill.textura_ligada;
        let Some(t) = self.texturas.get(&ligada) else {
            return;
        };
        let (tw, th) = (t.largura as f32, t.altura as f32);
        if tw == 0.0 || th == 0.0 || width == 0.0 || height == 0.0 {
            return;
        }
        let [ucr, vcr, wcr, hcr] = t.crop.map(|v| v as f32);
        // Recorte zerado é a textura inteira, como no rasterizador de software.
        let (wcr, hcr) = match (wcr, hcr) {
            (0.0, 0.0) => (tw, th),
            _ => (wcr, hcr),
        };
        let (s0, s1) = (ucr / tw, (ucr + wcr) / tw);
        let (t0, t1) = (vcr / th, (vcr + hcr) / th);
        let altura_superficie = self.surface().1 as f32;
        let (esquerda, direita) = (x, x + width);
        let (topo, base) = (altura_superficie - y - height, altura_superficie - y);
        let cor = self.current_color();
        let (vx, vy, vw, vh) = self.viewport_do_topo();
        if vw <= 0 || vh <= 0 {
            return;
        }
        let cantos = [
            ([esquerda, topo], [s0, t1]),
            ([esquerda, base], [s0, t0]),
            ([direita, base], [s1, t0]),
            ([direita, topo], [s1, t1]),
        ];
        self.vertices.clear();
        for ([sx, sy], uv) in cantos {
            let v = Vertex {
                normal: [0.0, 0.0, 1.0],
                uv1: [0.0; 2],
                fog: 1.0,
                position: [
                    ((sx - vx as f32) / vw as f32) * 2.0 - 1.0,
                    1.0 - ((sy - vy as f32) / vh as f32) * 2.0,
                    z * 2.0 - 1.0,
                    1.0,
                ],
                color: cor,
                uv,
            };
            self.poe(&v);
        }
        // A extensão não define orientação para o retângulo, então descartá-lo por face seria
        // descartar um desenho que o jogo espera ver.
        let guarda = self.fill.clone();
        self.fill.descarte = false;
        self.fill.texturando = true;
        self.submete_com(glow::TRIANGLE_FAN, -1.0, None);
        self.fill = guarda;
    }

    /// **O quadro da placa está guardado com a linha 0 no topo**, porque o Y é virado no shader
    /// de vértice — ver [`VERTICE`]. O `glReadPixels` do jogo, porém, conta o `y` de baixo e
    /// espera a primeira linha do resultado sendo a de baixo, que é o que o rasterizador de
    /// software entrega. Ler cru devolvia a imagem **de cabeça para baixo**: era a foto do
    /// Zeeboids salva invertida, e com ela o rosto do boneco de ponta-cabeça em todo jogo que o
    /// usa depois.
    ///
    /// Então a faixa pedida é convertida para a linha correspondente do framebuffer e o
    /// resultado sai espelhado de volta.
    fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]> {
        self.descarrega();
        if width == 0 || height == 0 {
            return Vec::new();
        }
        self.liga_para_leitura();
        let altura_da_superficie = self.surface().1 as i32;
        let de_baixo = altura_da_superficie - y - height as i32;
        let mut bytes = vec![0u8; width * height * 4];
        unsafe {
            self.gl.read_pixels(
                x,
                de_baixo,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
        }
        self.devolve_o_contexto();
        let mut saida = Vec::with_capacity(width * height);
        for linha in bytes.chunks_exact(width * 4).rev() {
            saida.extend(linha.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]));
        }
        saida
    }

    fn quadro_espera_pela_placa(&self) -> bool {
        true
    }

    fn estado_enviado_e_poupado(&self) -> (u64, u64) {
        GpuState::estado_enviado_e_poupado(self)
    }

    fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>) {
        self.descarrega();
        // **A placa morreu no meio do quadro: a leitura não acontece.** `glReadPixels` é uma
        // chamada como as outras, e depois do aviso as funções que este estado guardou não
        // existem. Sai um quadro preto do tamanho pedido — um só, até a sessão nascer de novo no
        // quadro seguinte (ver [`Placa::morreu`] e a nota de corrida em `frontends/libretro`).
        if self.placa.morreu() {
            out.clear();
            out.resize(width * height * 2, 0);
            self.sujo = false;
            return;
        }
        // Mesmo atalho do rasterizador de software: quadro igual ao que já está em `out` não tem
        // o que reconverter. Na placa isso vale ainda mais, porque a leitura é uma ida e volta.
        if !self.sujo && out.len() == width * height * 2 {
            return;
        }
        let (sw, sh) = self.surface();
        if sw == 0 || sh == 0 {
            return;
        }
        // **A placa entrega o quadro já em RGB565.** Converter RGBA na CPU custava 1,8 ms por
        // quadro no Need for Speed — seis vezes a leitura em si —, e a 60 quadros por segundo era
        // a maior fatia do `eglSwapBuffers`. No mesmo tamanho é uma cópia; com reamostragem, o
        // índice de cada coluna sai uma vez por quadro, e não uma vez por pixel.
        let em_565 = self.le_quadro_rgb565(sw, sh);
        out.clear();
        out.resize(width * height * 2, 0);
        if em_565 && sw == width && sh == height {
            out.copy_from_slice(&self.pixels[..width * height * 2]);
        } else {
            let (passo, converte): (usize, fn(&[u8]) -> [u8; 2]) = match em_565 {
                true => (2, |p| [p[0], p[1]]),
                false => (4, |p| {
                    (((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3))
                        .to_le_bytes()
                }),
            };
            let colunas: Vec<usize> = (0..width)
                .map(|x| (x * sw / width).min(sw - 1) * passo)
                .collect();
            for (y, saida) in out.chunks_exact_mut(width * 2).enumerate() {
                let inicio = (y * sh / height).min(sh - 1) * sw * passo;
                let linha = &self.pixels[inicio..inicio + sw * passo];
                for (par, &coluna) in saida.chunks_exact_mut(2).zip(&colunas) {
                    par.copy_from_slice(&converte(&linha[coluna..coluna + passo]));
                }
            }
        }
        self.sujo = false;
    }

    fn define_descarte_de_tiles(&mut self, descartar: bool) {
        self.descarta_tiles = descartar;
        crate::registro!(
            crate::registro::Nivel::Informacao,
            "gl",
            "descarte de profundidade e estêncil depois do quadro: {}",
            match descartar {
                true => "ligado (experimental, para GPU de tiles)",
                false => "desligado",
            }
        );
    }

    fn frame_rgb565_words(&mut self, width: usize, height: usize, out: &mut Vec<u16>) {
        self.descarrega();
        // Placa morta: nem esta leitura toca nela. Ver [`GpuState::frame_rgb565`].
        if self.placa.morreu() {
            out.clear();
            out.resize(width * height, 0);
            self.sujo = false;
            return;
        }
        if !self.sujo && out.len() == width * height {
            return;
        }
        let (sw, sh) = self.surface();
        if sw == 0 || sh == 0 {
            return;
        }
        let em_565 = self.le_quadro_rgb565(sw, sh);
        out.clear();
        out.resize(width * height, 0);
        let pixel = |p: &[u8]| -> u16 {
            if em_565 {
                u16::from_le_bytes([p[0], p[1]])
            } else {
                ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)
            }
        };
        let passo = if em_565 { 2 } else { 4 };
        let colunas: Vec<usize> = (0..width)
            .map(|x| (x * sw / width).min(sw - 1) * passo)
            .collect();
        for (y, saida) in out.chunks_exact_mut(width).enumerate() {
            let inicio = (y * sh / height).min(sh - 1) * sw * passo;
            let linha = &self.pixels[inicio..inicio + sw * passo];
            for (destino, &coluna) in saida.iter_mut().zip(&colunas) {
                *destino = pixel(&linha[coluna..coluna + passo]);
            }
        }
        self.sujo = false;
        // **O descarte dos anexos que ninguém vai ler.** O quadro de cor acabou de ser lido para
        // a memória da CPU, e a profundidade e o estêncil deste quadro não são precisos para
        // desenhar o próximo — desde que o jogo os limpe, que é o caso comum.
        //
        // **É experimental e vem desligado**, porque um jogo que **não** limpe a profundidade
        // conta com ela de um quadro para o outro — o console é um framebuffer de verdade, e a
        // profundidade de lá persiste. Num GPU de tiles, que é o caso do Mali dos portáteis,
        // dizer isto ao driver evita escrever os anexos de volta na memória: rende lá, e não no
        // desktop, que é onde ele não pode ser medido.
        if self.descarta_tiles && self.fbo_externo.is_none() {
            let gl = &self.gl;
            unsafe {
                gl.invalidate_framebuffer(
                    glow::FRAMEBUFFER,
                    &[glow::DEPTH, glow::STENCIL],
                );
            }
        }
    }

    fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]) {
        self.descarrega();
        let (sw, sh) = self.surface();
        if width == 0 || height == 0 || old.len() != width * height * 2 || new.len() != old.len() {
            return;
        }
        let (fw, fh) = self.frame_size();
        // **A fonte e o destino podem ter tamanhos diferentes, e quase sempre têm.** Na Z-Wheel o
        // pedido é de 640x330 — o pbuffer — e a superfície é 640x480. O rasterizador de software
        // cai num laço com escala e estica a fonte sobre a superfície; desenhar 1:1 nas 330
        // primeiras linhas comprimia tudo em 0..226 depois da reamostragem do quadro.
        //
        // Aqui quem estica é o amostrador da placa, de graça: a textura é a fonte inteira e o
        // retângulo cobre a superfície.
        let (colunas, linhas) = (width, height);
        let destino_x = sw.min(fw);
        let destino_y = sh.min(fh);
        if colunas == 0 || linhas == 0 || destino_x == 0 || destino_y == 0 {
            return;
        }
        // **O alfa carrega a máscara.** O rasterizador de software escreve só os pixels que
        // mudaram e preserva o alfa do destino; aqui o mesmo efeito sai numa transferência só:
        // alfa 255 onde mudou, 0 onde não, o teste de alfa descarta o resto e o `glColorMask`
        // fecha o canal de alfa para não sobrescrever o do destino.
        self.pixels.clear();
        self.pixels.resize(colunas * linhas * 4, 0);
        let mut mudou = false;
        for y in 0..linhas {
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
                let [r, g, b] = expande565(new, offset);
                let destino = (y * colunas + x) * 4;
                self.pixels[destino..destino + 4].copy_from_slice(&[r, g, b, 255]);
                mudou = true;
            }
        }
        if !mudou {
            return;
        }
        self.destino();
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.ponte));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                colunas as i32,
                linhas as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&self.pixels)),
            );
            for (nome, valor) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32),
                (glow::TEXTURE_MAX_LEVEL, 0),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, nome, valor);
            }
        }
        // O retângulo cobre a superfície dentro do quadro. As coordenadas já saem prontas, então
        // o shader não vira o Y: `virar` vale +1.
        let x1 = (destino_x as f32 / fw as f32) * 2.0 - 1.0;
        let y1 = (destino_y as f32 / fh as f32) * 2.0 - 1.0;
        self.vertices.clear();
        for ([px, py], uv) in [
            ([-1.0f32, -1.0f32], [0.0f32, 0.0f32]),
            ([x1, -1.0], [1.0, 0.0]),
            ([x1, y1], [1.0, 1.0]),
            ([-1.0, y1], [0.0, 1.0]),
        ] {
            self.poe(&Vertex {
                position: [px, py, 0.0, 1.0],
                color: [1.0; 4],
                uv,
                normal: [0.0, 0.0, 1.0],
                uv1: [0.0; 2],
                fog: 1.0,
            });
        }
        let guarda = self.fill.clone();
        self.fill.viewport_do_topo_fixa = Some((0, 0, fw as i32, fh as i32));
        self.fill.teste_profundidade = false;
        self.fill.teste_stencil = false;
        self.fill.mistura = false;
        self.fill.descarte = false;
        self.fill.mascara_profundidade = false;
        self.fill.mascara_cor = [true, true, true, false];
        self.fill.env_textura = TexEnv::com_modo(gles::GL_REPLACE);
        self.fill.teste_alfa = true;
        self.fill.func_alfa = gles::GL_GREATER;
        self.fill.ref_alfa = 0.5;
        let ponte = self.ponte;
        self.submete_com(glow::TRIANGLE_FAN, 1.0, Some(ponte));
        self.fill = guarda;
        self.devolve_o_contexto();
    }

    fn define_escala(&mut self, escala: usize) {
        // Sem blit confiável não há redução do quadro grande, e a escala é justamente isso: o
        // desenho maior reduzido antes de qualquer leitura. Ficar em 1x é a resposta certa.
        if !self.blit_confiavel {
            return;
        }
        self.descarrega();
        // O teto é o maior anexo que a placa aceita: um fator acima dele não criaria o destino.
        let maximo = unsafe {
            self.gl
                .get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE)
                .min(self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE))
        }
        .max(1) as usize;
        let (fw, fh) = self.estado.frame_size();
        let cabe = (maximo / fw.max(fh).max(1)).max(1);
        let escala = escala.clamp(1, cabe);
        if escala != self.escala {
            self.escala = escala;
            self.sujo = true;
        }
    }

    fn le_quadro_grande(&mut self) -> Option<(usize, usize, Vec<u8>)> {
        self.descarrega();
        let extra = self.extra();
        if self.escala <= 1 && extra == 0 {
            return None;
        }
        self.destino();
        self.resolve();
        let (sw, sh) = self.estado.surface();
        let (w, h) = ((sw + 2 * extra) * self.escala, sh * self.escala);
        let mut bytes = vec![0u8; w * h * 4];
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
        }
        self.devolve_o_contexto();
        Some((w, h, bytes))
    }

    fn define_proporcao(&mut self, aspecto: Option<f32>) {
        self.descarrega();
        // Mais estreito que o nativo não abre nada; o teto evita um anexo absurdo.
        let aspecto = aspecto.filter(|a| a.is_finite()).map(|a| a.clamp(4.0 / 3.0, 3.6));
        if aspecto != self.proporcao {
            self.proporcao = aspecto;
            self.sujo = true;
        }
    }

    fn define_antialias(&mut self, amostras: usize) {
        // O `resolve` do MSAA é um blit entre framebuffers. Sem blit confiável, pedir amostras
        // daria borda suja em vez de borda suave.
        if !self.blit_confiavel {
            return;
        }
        self.descarrega();
        let maximo = unsafe { self.gl.get_parameter_i32(glow::MAX_SAMPLES) }.max(1) as usize;
        // Potências de dois são o que as placas oferecem; 1 é desligado.
        let pedido = match amostras {
            0 | 1 => 1,
            n => n.next_power_of_two().min(maximo.max(1)),
        };
        if pedido != self.amostras {
            self.amostras = pedido;
            self.sujo = true;
        }
    }

    fn define_anisotropico(&mut self, nivel: usize) {
        self.descarrega();
        let tem = self.gl.supported_extensions().iter().any(|e| {
            e == "GL_EXT_texture_filter_anisotropic" || e == "GL_ARB_texture_filter_anisotropic"
        });
        let maximo = match tem {
            true => unsafe { self.gl.get_parameter_f32(MAX_TEXTURE_MAX_ANISOTROPY) }.max(1.0),
            false => 1.0,
        };
        let nivel = (nivel.max(1) as f32).min(maximo);
        if nivel == self.anisotropia {
            return;
        }
        // Voltar a 1 também precisa ser escrito nas texturas: o parâmetro fica nelas.
        let reescrever_para_um = nivel <= 1.0 && self.anisotropia > 1.0;
        self.anisotropia = nivel;
        let gl = self.gl.clone();
        for t in self.texturas.values() {
            self.parametros(t);
            if reescrever_para_um {
                unsafe { gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, 1.0) };
            }
        }
        unsafe { gl.bind_texture(glow::TEXTURE_2D, None) };
    }

    fn quadro_na_placa(&self) -> Option<QuadroNaPlaca> {
        let destino = self.quadro.as_ref()?;
        if destino.escala <= 1 && destino.extra == 0 {
            return None;
        }
        let (fw, fh) = destino.medida;
        let (sw, sh) = self.estado.surface();
        let extra = destino.extra;
        Some(QuadroNaPlaca {
            textura: destino.cor,
            recorte: [
                (sw.min(fw) + 2 * extra) as f32 / (fw + 2 * extra).max(1) as f32,
                sh.min(fh) as f32 / fh.max(1) as f32,
            ],
            proporcao: match self.estado.superficie_esticada() {
                // Esticada, a superfície ocupa a tela: a altura é a da tela, e a largura é a
                // dela mais as colunas a mais, na escala do esticamento.
                true => (sw + 2 * extra) as f32 / sw.max(1) as f32 * fw as f32 / fh.max(1) as f32,
                false => (sw.min(fw) + 2 * extra) as f32 / sh.min(fh).max(1) as f32,
            },
        })
    }
}

/// A tesoura do jogo nos pixels do anexo, ainda sem a escala.
///
/// **O `y` vira contado do topo, como o da viewport.** O `glScissor` conta de baixo para cima e o
/// anexo guarda a imagem de cima para baixo; a viewport passa por `viewport_do_topo` e a tesoura
/// ia crua. Na tela inteira (`0 0 640 480`) as duas leituras coincidem, e por isso o erro só
/// aparecia em retângulos: o Crash Nitro Kart desenha o trecho seguinte da pista por um portal,
/// com viewport e tesoura no retângulo dele, e a tesoura caía na faixa espelhada da tela — o
/// portal saía vazio e o cenário "subia do nada" quando o kart o atravessava. É o mesmo sintoma
/// que a viewport já teve, e voltou quando a tesoura passou a ser respeitada na placa.
///
/// **Na proporção larga, a tesoura se desloca, e só cresce até as bordas que já tocava.** O que
/// estava na tela cai no mesmo pixel, deslocado de `extra`; uma tesoura na tela inteira — a que o
/// Resident Evil 4 e o Crash Nitro Kart ligam em jogo — tem de ganhar os lados novos, ou eles
/// ficam com a cor de fundo do anexo. Alargá-la pela razão da viewport, como antes, fazia o
/// retângulo de um portal vazar para fora da moldura dele.
fn tesoura_no_anexo(
    (x, y, largura, altura): (i32, i32, i32, i32),
    (largura_da_superficie, altura_da_superficie): (usize, usize),
    extra: i32,
) -> (i32, i32, i32, i32) {
    let (s_largura, s_altura) = (largura_da_superficie as i32, altura_da_superficie as i32);
    let topo = s_altura - y - altura;
    let esquerda = match x <= 0 {
        true => 0,
        false => x + extra,
    };
    let direita = match x + largura >= s_largura {
        true => s_largura + 2 * extra,
        false => x + largura + extra,
    };
    (esquerda, topo, direita - esquerda, altura)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Abre os dois rasterizadores no mesmo tamanho, ou desiste quando não há placa.
    ///
    /// Sem contexto não há o que comparar, e exigir uma placa de quem roda a suíte seria pedir
    /// que ela falhasse em máquina sem EGL. Os testes daqui relatam e passam nesse caso.
    fn par(largura: usize, altura: usize) -> Option<(GpuState, GlState)> {
        match GpuState::novo(largura, altura, None) {
            Ok(gpu) => Some((gpu, GlState::new(largura, altura))),
            Err(motivo) => {
                println!("sem placa nesta máquina: {motivo}");
                None
            }
        }
    }

    /// O restore da placa precisa recriar os objetos reais de textura e as duas unidades.
    ///
    /// RE4 usa a unidade 1 para combinar a textura base com a segunda camada. O estado de software
    /// sempre guardou as duas, mas a 0.3.0 limpava o cache de objetos da GPU e esquecia de copiar a
    /// unidade 1 para o espelho de desenho; o resultado era a cena branca depois de Load.
    #[test]
    fn save_state_recria_texturas_e_a_segunda_unidade() {
        let Some((mut gpu, _)) = par(16, 16) else {
            return;
        };
        // **A unidade vem como enum do GL, não como índice.** `set_active_texture` recebe o que o
        // guest manda no `glActiveTexture`, e o estado guarda `enum - GL_TEXTURE0`. Passando `1`,
        // a subtração dá um número enorme, a unidade 1 nunca é preparada — e a prova falhava
        // acusando o código de perder o que ela mesma não tinha posto. Este teste ficou vermelho
        // em `development` por isso: ele nasceu no PR dos save states do Android, cuja CI foi
        // cancelada antes de rodá-lo.
        gpu.set_active_texture(gles::GL_TEXTURE0);
        gpu.bind_texture(10);
        gpu.upload_level(10, 0, 1, 1, vec![[255, 0, 0, 255]]);
        gpu.set_capability(gles::GL_TEXTURE_2D, true);
        gpu.set_active_texture(gles::GL_TEXTURE0 + 1);
        gpu.bind_texture(20);
        gpu.upload_level(20, 0, 1, 1, vec![[0, 255, 0, 255]]);
        gpu.set_capability(gles::GL_TEXTURE_2D, true);

        let mut secoes = crate::save_state::Secoes::nova();
        gpu.grava_estado(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = crate::save_state::Leitor::abre(&arquivo).unwrap();

        gpu.delete_texture(10);
        gpu.delete_texture(20);
        gpu.restaura_estado(&leitor).unwrap();

        assert!(gpu.texturas.contains_key(&10), "a textura base nao voltou para a placa");
        assert!(gpu.texturas.contains_key(&20), "a textura da unidade 1 nao voltou para a placa");
        assert!(gpu.fill.unidade1.ligada, "a unidade 1 perdeu o GL_TEXTURE_2D");
        assert_eq!(gpu.fill.unidade1.textura, 20, "a unidade 1 voltou com outro nome");
    }

    /// Desenha a mesma coisa nos dois e devolve os quadros em RGB565, para comparar.
    fn ambos(
        gpu: &mut GpuState,
        sw: &mut GlState,
        largura: usize,
        altura: usize,
        cena: impl Fn(&mut dyn Rasterizador),
    ) -> (Vec<u8>, Vec<u8>) {
        let mut a = Vec::new();
        let mut b = Vec::new();
        cena(gpu);
        cena(sw);
        gpu.frame_rgb565(largura, altura, &mut a);
        sw.frame_rgb565(largura, altura, &mut b);
        (a, b)
    }

    /// O pixel `(x, y)` de um quadro RGB565, como `(r, g, b)` de cinco/seis bits.
    fn pixel(quadro: &[u8], largura: usize, x: usize, y: usize) -> (u16, u16, u16) {
        let i = (y * largura + x) * 2;
        let v = u16::from_le_bytes([quadro[i], quadro[i + 1]]);
        ((v >> 11) & 31, (v >> 5) & 63, v & 31)
    }

    /// Um triângulo que cobre o canto superior esquerdo, para pegar orientação e preenchimento.
    ///
    /// É o teste que separa "a placa não desenhou" de "a placa desenhou no lugar errado" — e o
    /// lugar errado é o erro mais fácil de cometer aqui, porque o OpenGL conta linhas de baixo
    /// para cima e a superfície do console, de cima para baixo.
    #[test]
    fn o_triangulo_cai_no_mesmo_canto_nos_dois_rasterizadores() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // Em espaço de recorte: canto superior esquerdo é (-1, +1).
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                uv1: [0.0; 2],
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);

        let canto_placa = pixel(&a, largura, 1, 1);
        let canto_software = pixel(&b, largura, 1, 1);
        assert_eq!(
            canto_software.0, 31,
            "o software devia pintar o canto de cima de vermelho"
        );
        assert_eq!(
            canto_placa, canto_software,
            "canto de cima: placa {canto_placa:?} contra software {canto_software:?}"
        );
        let baixo_placa = pixel(&a, largura, 14, 14);
        let baixo_software = pixel(&b, largura, 14, 14);
        assert_eq!(
            baixo_placa, baixo_software,
            "canto de baixo: placa {baixo_placa:?} contra software {baixo_software:?}"
        );
    }

    /// O `glReadPixels` conta o `y` de baixo, e os dois rasterizadores têm de concordar.
    ///
    /// A placa guarda o quadro com a linha 0 no topo, porque o Y é virado no shader; o software
    /// guarda igual, mas converte na leitura. Ler cru da placa devolvia a imagem de cabeça para
    /// baixo — era a foto do Zeeboids salva invertida, e com ela o rosto do boneco de
    /// ponta-cabeça em todo jogo que o usa depois.
    #[test]
    fn a_leitura_de_pixels_tem_a_mesma_orientacao_nos_dois_rasterizadores() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        // Metade de cima vermelha: um quadro que não é simétrico na vertical.
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 1.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                uv1: [0.0; 2],
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, 0.0), canto(1.0, 1.0)],
            );
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(1.0, 1.0), canto(-1.0, 0.0), canto(1.0, 0.0)],
            );
        };
        cena(&mut gpu);
        cena(&mut sw);
        let da_placa = gpu.read_rect(0, 0, largura, altura);
        let do_software = sw.read_rect(0, 0, largura, altura);
        // A primeira linha do resultado é a de **baixo** da tela, que aqui é azul.
        assert_eq!(
            do_software[largura + 1][2], 255,
            "no software a primeira linha lida devia ser a de baixo, azul"
        );
        assert_eq!(
            da_placa[largura + 1],
            do_software[largura + 1],
            "linha de baixo: placa {:?} contra software {:?}",
            da_placa[largura + 1],
            do_software[largura + 1]
        );
        let alto = (altura - 2) * largura + 1;
        assert_eq!(
            da_placa[alto], do_software[alto],
            "linha de cima: placa {:?} contra software {:?}",
            da_placa[alto], do_software[alto]
        );
    }

    /// A névoa tinge o fragmento igual nos dois rasterizadores, e some quando é desligada.
    ///
    /// O fator sai da distância em coordenadas de olho e vem da etapa de vértice, que é comum
    /// aos dois; o que se compara aqui é o que cada um faz com ele no fragmento.
    #[test]
    fn a_nevoa_tinge_igual_nos_dois_rasterizadores() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        // Um quadrado vermelho a dez unidades do olho, com névoa azul de 0 a 20: metade do
        // caminho, então metade da cor de cada um.
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // Uma ortográfica de 1 a 100 em `z`, para o quadrado a dez unidades do olho caber
            // no recorte sem mexer em `x` e `y`.
            r.set_matrix_mode(gles::GL_PROJECTION);
            r.load_identity();
            let (perto, longe) = (1.0f32, 100.0f32);
            let mut orto = crate::video::rasterizer::IDENTITY;
            orto[10] = -2.0 / (longe - perto);
            orto[14] = -(longe + perto) / (longe - perto);
            r.mult_matrix(orto);
            r.set_matrix_mode(gles::GL_MODELVIEW);
            r.load_identity();
            r.mult_matrix(crate::video::rasterizer::translation(0.0, 0.0, -10.0));
            r.set_fog(gles::GL_FOG_MODE, [gles::GL_LINEAR as f32, 0.0, 0.0, 0.0]);
            r.set_fog(gles::GL_FOG_START, [0.0; 4]);
            r.set_fog(gles::GL_FOG_END, [20.0, 0.0, 0.0, 0.0]);
            r.set_fog(gles::GL_FOG_COLOR, [0.0, 0.0, 1.0, 1.0]);
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                uv1: [0.0; 2],
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            // Em espaço de recorte a projeção é a identidade, então o `z` do olho é o da
            // translação: dez unidades, metade do caminho até o fim da névoa.
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let com = |r: &mut dyn Rasterizador| {
            r.set_capability(gles::GL_FOG, true);
            cena(r);
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, com);
        let (placa, software) = (pixel(&a, largura, 1, 1), pixel(&b, largura, 1, 1));
        assert!(
            software.0 > 10 && software.0 < 24 && software.2 > 5,
            "o software devia misturar vermelho e azul, e deu {software:?}"
        );
        assert!(
            placa.0.abs_diff(software.0) <= 1 && placa.2.abs_diff(software.2) <= 1,
            "com névoa: placa {placa:?} contra software {software:?}"
        );
        // Desligada por quem joga, a cor volta a ser a do jogo nos dois.
        let sem = |r: &mut dyn Rasterizador| {
            r.define_neblina(false);
            r.set_capability(gles::GL_FOG, true);
            cena(r);
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, sem);
        assert_eq!(
            pixel(&b, largura, 1, 1),
            (31, 0, 0),
            "sem névoa o software devia deixar o vermelho do jogo"
        );
        assert_eq!(
            pixel(&a, largura, 1, 1),
            pixel(&b, largura, 1, 1),
            "sem névoa os dois têm de voltar a concordar"
        );
    }

    /// Com resolução interna maior, o jogo lê de volta o mesmo quadro de sempre.
    ///
    /// O desenho acontece num anexo `escala` vezes maior, e a leitura passa por uma redução na
    /// placa. Longe das bordas o pixel tem de ser o do quadro nativo; nas bordas a redução mistura
    /// vizinhos, e é justamente isso que suaviza — por isso só os interiores são comparados. A
    /// textura para a janela tem de existir, e o recorte, cobrir a superfície inteira.
    #[test]
    fn com_escala_a_leitura_continua_no_tamanho_do_console() {
        let (largura, altura) = (16, 16);
        let Some((mut nativa, mut sw)) = par(largura, altura) else {
            return;
        };
        let Ok(mut grande) = GpuState::novo(largura, altura, None) else {
            return;
        };
        grande.define_escala(2);
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 1.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                uv1: [0.0; 2],
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let (a, _) = ambos(&mut nativa, &mut sw, largura, altura, cena);
        cena(&mut grande);
        let mut b = Vec::new();
        grande.frame_rgb565(largura, altura, &mut b);
        assert_eq!(b.len(), a.len(), "a leitura sai no tamanho do console");
        for (x, y) in [(1, 1), (2, 3), (14, 14), (13, 12)] {
            assert_eq!(
                pixel(&b, largura, x, y),
                pixel(&a, largura, x, y),
                "pixel ({x}, {y}) com escala 2"
            );
        }
        let quadro = grande.quadro_na_placa().expect("textura grande para a janela");
        assert_eq!(quadro.recorte, [1.0, 1.0]);
        assert!(nativa.quadro_na_placa().is_none(), "na escala 1 a janela usa a tela de sempre");
    }

    /// Com antialias a borda do triângulo mistura as duas cores, e o miolo fica como estava.
    ///
    /// O desenho vai para um framebuffer de várias amostras e é resolvido antes da leitura. Na
    /// diagonal, algum pixel tem de sair com vermelho e azul ao mesmo tempo — é a prova de que as
    /// amostras chegaram à leitura; sem resolver, a leitura sairia preta ou falharia.
    #[test]
    fn com_antialias_a_diagonal_mistura_as_cores() {
        let (largura, altura) = (16, 16);
        let Ok(mut gpu) = GpuState::novo(largura, altura, None) else {
            return;
        };
        gpu.define_antialias(4);
        if gpu.amostras <= 1 {
            println!("placa sem amostragem múltipla");
            return;
        }
        gpu.set_viewport(0, 0, largura as i32, altura as i32);
        gpu.set_clear_color([0.0, 0.0, 1.0, 1.0]);
        gpu.clear(gles::GL_COLOR_BUFFER_BIT);
        let canto = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0, 1.0],
            color: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            uv1: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
                fog: 1.0,
        };
        gpu.draw(
            gles::GL_TRIANGLES,
            &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
        );
        let mut quadro = Vec::new();
        gpu.frame_rgb565(largura, altura, &mut quadro);
        assert_eq!(quadro.len(), largura * altura * 2);
        assert_eq!(pixel(&quadro, largura, 1, 1), (31, 0, 0), "miolo vermelho");
        assert_eq!(pixel(&quadro, largura, 14, 14), (0, 0, 31), "miolo azul");
        let misturado = (0..largura).any(|x| {
            let (r, _, b) = pixel(&quadro, largura, x, largura - 1 - x);
            r > 0 && b > 0
        });
        assert!(misturado, "algum pixel da diagonal devia misturar vermelho e azul");
    }

    /// A tesoura de um retângulo cai no mesmo lugar do retângulo, com o `y` contado do topo.
    ///
    /// O portal do Crash Nitro Kart é uma viewport e uma tesoura num retângulo; com o `y` cru a
    /// tesoura caía na faixa espelhada e o portal saía vazio.
    #[test]
    fn a_tesoura_de_um_retangulo_conta_o_y_do_topo() {
        // 42×64 a 21 da esquerda e 42 de baixo, numa tela de 640×480: o topo fica em 374.
        assert_eq!(tesoura_no_anexo((21, 42, 42, 64), (640, 480), 0), (21, 374, 42, 64));
        // Na proporção larga ele só se desloca; a tela inteira ganha os lados.
        assert_eq!(tesoura_no_anexo((21, 42, 42, 64), (640, 480), 80), (101, 374, 42, 64));
        assert_eq!(tesoura_no_anexo((0, 0, 640, 480), (640, 480), 80), (0, 0, 800, 480));
    }

    /// Na proporção larga, a tesoura do jogo não pode cortar os lados novos.
    ///
    /// O `glScissor` chega em pixels do console, e o anexo é mais largo: sem a mesma conversão
    /// que a viewport recebe, uma tesoura na tela inteira — que é o que o Resident Evil 4 e o
    /// Crash Nitro Kart ligam em jogo — cortava tudo além dos 640 do console, e os lados
    /// ficavam com a cor de fundo do anexo.
    #[test]
    fn na_proporcao_larga_a_tesoura_do_jogo_nao_come_os_lados() {
        let (largura, altura) = (64, 48);
        let Ok(mut gpu) = GpuState::novo(largura, altura, None) else {
            println!("sem placa nesta máquina");
            return;
        };
        gpu.define_proporcao(Some(16.0 / 9.0));
        gpu.set_viewport(0, 0, largura as i32, altura as i32);
        // Uma perspectiva qualquer: é ela que faz o lote ganhar os lados.
        gpu.set_matrix_mode(gles::GL_PROJECTION);
        gpu.load_identity();
        let mut perspectiva = crate::video::rasterizer::IDENTITY;
        perspectiva[11] = -1.0;
        perspectiva[15] = 0.0;
        perspectiva[10] = -1.0;
        perspectiva[14] = -2.0;
        gpu.mult_matrix(perspectiva);
        gpu.set_matrix_mode(gles::GL_MODELVIEW);
        gpu.load_identity();
        // A tesoura da tela inteira, em pixels do console.
        gpu.set_scissor(0, 0, largura as i32, altura as i32);
        gpu.set_capability(gles::GL_SCISSOR_TEST, true);
        gpu.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        gpu.clear(gles::GL_COLOR_BUFFER_BIT);
        // Um quadrado bem maior que a tela, para cobrir também os lados novos.
        let canto = |x: f32, y: f32| Vertex {
            position: [x, y, -1.0, 1.0],
            color: [0.0, 1.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            uv1: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            fog: 1.0,
        };
        for tri in [
            [canto(-4.0, 4.0), canto(-4.0, -4.0), canto(4.0, 4.0)],
            [canto(4.0, 4.0), canto(-4.0, -4.0), canto(4.0, -4.0)],
        ] {
            gpu.draw(gles::GL_TRIANGLES, &tri);
        }
        let (w, h, rgba) = gpu.le_quadro_grande().expect("o quadro largo existe");
        assert!(w > largura, "a proporção larga devia alargar o anexo");
        let verde = |x: usize| {
            let i = ((h / 2) * w + x) * 4;
            rgba[i + 1]
        };
        assert!(verde(w / 2) > 200, "o centro devia estar pintado");
        assert!(
            verde(w - 2) > 200,
            "a borda direita ficou em {} — a tesoura comeu o lado novo",
            verde(w - 2)
        );
        assert!(verde(1) > 200, "a borda esquerda ficou em {}", verde(1));
    }

    /// Quem nunca chama `glViewport` tem que desenhar de todo jeito.
    ///
    /// A Z-Wheel é assim — zero chamadas em treze segundos —, e foi este o defeito que apagou a
    /// roda, o chão e o diálogo dela na primeira execução na placa: a viewport nascia em zero.
    #[test]
    fn sem_glviewport_a_placa_desenha_na_tela_inteira() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        let cena = |r: &mut dyn Rasterizador| {
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [0.0, 1.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                uv1: [0.0; 2],
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);
        assert_eq!(
            pixel(&b, largura, 1, 1).1,
            63,
            "o software devia pintar o canto de verde sem viewport nenhuma"
        );
        assert_eq!(
            pixel(&a, largura, 1, 1),
            pixel(&b, largura, 1, 1),
            "sem viewport: placa {:?} contra software {:?}",
            pixel(&a, largura, 1, 1),
            pixel(&b, largura, 1, 1)
        );
    }

    /// Geometria além do plano distante: o software desenha, e o OpenGL recortaria.
    ///
    /// O rasterizador de software só recorta no plano **próximo**; o OpenGL recorta nos seis
    /// planos do frustum. Um jogo que passe do plano distante — e a Z-Wheel passa — veria a
    /// superfície simplesmente desaparecer na placa. É o que o `GL_DEPTH_CLAMP` resolve: em vez
    /// de recortar em profundidade, ele prende o valor na faixa.
    #[test]
    fn o_que_passa_do_plano_distante_aparece_nos_dois() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // Z além de 1 em espaço de recorte: fora do frustum pelo plano distante.
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 1.5, 1.0],
                color: [0.0, 0.0, 1.0, 1.0],
                uv: [0.0, 0.0],
                uv1: [0.0; 2],
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);
        assert_eq!(
            pixel(&b, largura, 2, 2).2,
            31,
            "o software devia desenhar, porque não recorta no plano distante"
        );
        assert_eq!(
            pixel(&a, largura, 2, 2),
            pixel(&b, largura, 2, 2),
            "além do plano distante: placa {:?} contra software {:?}",
            pixel(&a, largura, 2, 2),
            pixel(&b, largura, 2, 2)
        );
    }

    /// O descarte por face tem que concordar nos dois, nas quatro combinações.
    ///
    /// É o ponto onde as convenções se cruzam: o rasterizador de software decide em coordenadas
    /// de tela e chama CCW o caso de área **negativa** (linha do `counter_clockwise`), enquanto o
    /// OpenGL chama CCW o de área positiva. Como o Y é virado no shader, as coordenadas dos dois
    /// são numericamente as mesmas — então a face frontal precisa ser invertida ao entrar na
    /// placa. Este teste é o que prova que a inversão está no sentido certo, e não o contrário.
    #[test]
    fn o_descarte_por_face_concorda_nos_dois() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        for frente in [gles::GL_CCW, gles::GL_CW] {
            for invertido in [false, true] {
                let cena = |r: &mut dyn Rasterizador| {
                    r.set_viewport(0, 0, largura as i32, altura as i32);
                    r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
                    r.clear(gles::GL_COLOR_BUFFER_BIT);
                    r.set_capability(gles::GL_CULL_FACE, true);
                    r.set_cull_face(gles::GL_BACK);
                    r.set_front_face(frente);
                    let canto = |x: f32, y: f32| Vertex {
                        position: [x, y, 0.0, 1.0],
                        color: [1.0, 1.0, 1.0, 1.0],
                        uv: [0.0, 0.0],
                        uv1: [0.0; 2],
                        normal: [0.0, 0.0, 1.0],
                fog: 1.0,
                    };
                    // Um triângulo que cobre o centro, nas duas ordens de vértice.
                    let tri = match invertido {
                        false => [canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
                        true => [canto(1.0, 1.0), canto(-1.0, -1.0), canto(-1.0, 1.0)],
                    };
                    r.draw(gles::GL_TRIANGLES, &tri);
                };
                let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);
                let na_placa = pixel(&a, largura, 2, 2);
                let no_software = pixel(&b, largura, 2, 2);
                assert_eq!(
                    na_placa, no_software,
                    "frente={frente:#x} invertido={invertido}: placa {na_placa:?} contra \
                     software {no_software:?}"
                );
            }
        }
    }

    /// O `import_rgb565_changes` é por onde o 2D do jogo entra no palco: é ele que leva o diálogo
    /// e as capas da Z-Wheel para dentro do quadro do OpenGL.
    #[test]
    fn o_import_poe_os_pixels_do_jogo_no_mesmo_lugar() {
        let (largura, altura) = (8, 8);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        // Um quadro todo preto, e um segundo com a primeira linha branca: só ela deve mudar.
        let velho = vec![0u8; largura * altura * 2];
        let mut novo = velho.clone();
        for x in 0..largura {
            novo[x * 2] = 0xff;
            novo[x * 2 + 1] = 0xff;
        }
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
        };
        cena(&mut gpu);
        cena(&mut sw);
        gpu.import_rgb565_changes(largura, altura, &velho, &novo);
        sw.import_rgb565_changes(largura, altura, &velho, &novo);
        let mut a = Vec::new();
        let mut b = Vec::new();
        gpu.frame_rgb565(largura, altura, &mut a);
        sw.frame_rgb565(largura, altura, &mut b);

        assert_eq!(
            pixel(&b, largura, 3, 0),
            (31, 63, 31),
            "o software devia ter a primeira linha branca"
        );
        assert_eq!(
            pixel(&a, largura, 3, 0),
            pixel(&b, largura, 3, 0),
            "primeira linha: placa {:?} contra software {:?}",
            pixel(&a, largura, 3, 0),
            pixel(&b, largura, 3, 0)
        );
        assert_eq!(
            pixel(&a, largura, 3, 5),
            pixel(&b, largura, 3, 5),
            "linha do meio devia ter ficado preta nos dois"
        );
    }

    /// Um endereço que placa nenhuma tem, para as provas que não abrem contexto nenhum.
    const ENDERECO_FALSO: usize = usize::MAX;
    /// O endereço de uma segunda placa que também não existe.
    const OUTRO_ENDERECO: usize = usize::MAX - 8;

    /// **A placa que morreu larga os nomes de GL sem apagar nenhum deles.**
    ///
    /// Um contexto de GL não existe em teste unitário — e é por isso que as decisões que dependem
    /// dele moram na [`Placa`], que se prova sem placa nenhuma. O que se cobra aqui é o que o `Drop`
    /// pergunta antes de chamar `delete_*` (auditoria de GPU, achado 1): depois do aviso de que o
    /// contexto acabou, **nenhum** apagamento acontece — e depois de uma placa nova, volta a
    /// acontecer.
    #[test]
    fn a_placa_que_morreu_larga_os_nomes_e_a_que_nasceu_apaga() {
        // Placa emprestada e viva: o `Drop` apaga, como sempre apagou.
        let viva = Placa::nova(true, ENDERECO_FALSO);
        assert!(
            viva.apaga_ao_morrer(),
            "placa viva: o `Drop` apaga os nomes que criou"
        );

        // O aviso de que o contexto morreu — é a mesma chamada que o `contexto_perdido` e o
        // `contexto_reset` do core fazem.
        a_placa_morreu(ENDERECO_FALSO);
        assert!(viva.morreu(), "o estado não reconheceu o aviso da placa morta");
        assert!(
            !viva.apaga_ao_morrer(),
            "o `Drop` chamaria delete_* com os ponteiros de função de um contexto morto"
        );

        // **Recriada sob demanda:** uma placa nova, em outro endereço, volta a valer — a marca
        // antiga não a alcança.
        let nova = Placa::nova(true, OUTRO_ENDERECO);
        assert!(!nova.morreu());
        assert!(nova.apaga_ao_morrer());

        // E a que nasceu **no mesmo endereço** da morta — o alocador reaproveita endereços — também:
        // quem limpa a marca é `a_placa_nasceu`, que é o que o `liga_a_placa` do core chama.
        a_placa_nasceu(ENDERECO_FALSO);
        let renascida = Placa::nova(true, ENDERECO_FALSO);
        assert!(!renascida.morreu());
        assert!(renascida.apaga_ao_morrer());

        // Contexto próprio nunca morre antes do dono: quem o fecharia é este mesmo estado.
        a_placa_morreu(ENDERECO_FALSO);
        let propria = Placa::nova(false, ENDERECO_FALSO);
        assert!(!propria.morreu());
        assert!(
            propria.apaga_ao_morrer(),
            "contexto próprio: quem apaga os nomes é o `Drop` dele"
        );

        // A marca é do processo todo: deixá-la posta mudaria o teste que rodasse depois.
        a_placa_nasceu(ENDERECO_FALSO);
        a_placa_nasceu(OUTRO_ENDERECO);
    }

    /// **O cache dos três objetos morre quando o contexto sai das nossas mãos.**
    ///
    /// O `Pintor` da janela termina cada pintura **desligando** programa e VAO, e pinta no mesmo
    /// contexto em que o rasterizador desenha: sem invalidar, o lote seguinte desenharia com o
    /// programa do `egui` — ou com nenhum, que é `GL_INVALID_OPERATION` e nada na tela, sem erro
    /// nenhum do lado de cá (auditoria de GPU, achado 2).
    #[test]
    fn o_contexto_que_sai_das_maos_invalida_os_objetos_ligados() {
        let placa = Placa::nova(true, ENDERECO_FALSO);
        assert!(placa.precisa_ligar(), "o primeiro lote liga os três");
        placa.ligou();
        assert!(
            !placa.precisa_ligar(),
            "o lote seguinte reaproveita o que já está ligado — é o ganho do commit e6a436e"
        );
        placa.esquece_o_ligado();
        assert!(
            placa.precisa_ligar(),
            "o contexto foi devolvido ao anfitrião: o próximo lote tem de religar"
        );
    }

    /// Uma placa fora de tela para os testes que precisam de um contexto **emprestado**.
    ///
    /// É o caminho da janela e do frontend: o contexto é de outro, e é por isso que ele pode ser
    /// tomado de volta no meio do desenho. O `Contexto` volta junto porque é ele que mantém as
    /// funções de GL vivas: largá-lo antes do estado derrubaria o teste, como derruba o programa.
    #[cfg(feature = "gpu")]
    fn placa_emprestada() -> Option<(Contexto, std::sync::Arc<glow::Context>)> {
        match Contexto::novo() {
            Ok(contexto) => {
                let gl = contexto.gl.clone();
                // O endereço pode ter sido o de uma placa que outro teste marcou como morta: a
                // marca é do processo todo, e quem a limpa é quem monta a placa — aqui, e no
                // `liga_a_placa` do core.
                a_placa_nasceu(endereco_da_placa(&gl));
                Some((contexto, gl))
            }
            Err(porque) => {
                println!("sem placa fora de tela: {porque}");
                None
            }
        }
    }

    /// Um estado de placa sobre o contexto emprestado, como o do jogo na janela.
    #[cfg(feature = "gpu")]
    fn estado_emprestado(
        largura: usize,
        altura: usize,
        gl: &std::sync::Arc<glow::Context>,
    ) -> Option<GpuState> {
        match GpuState::novo(largura, altura, Some(gl.clone())) {
            Ok(estado) => Some(estado),
            Err(motivo) => {
                println!("sem estado de placa: {motivo}");
                None
            }
        }
    }

    /// Um triângulo da cena, em espaço de recorte, na cor dada.
    #[cfg(feature = "gpu")]
    fn triangulo(r: &mut dyn Rasterizador, cor: [f32; 4], cantos: [(f32, f32); 3]) {
        let vertice = |(x, y): (f32, f32)| Vertex {
            position: [x, y, 0.0, 1.0],
            color: cor,
            uv: [0.0, 0.0],
            uv1: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            fog: 1.0,
        };
        r.set_color(cor);
        r.draw(gles::GL_TRIANGLES, &cantos.map(vertice));
    }

    /// O primeiro lote da cena dos dois testes abaixo: um triângulo vermelho no canto de cima.
    #[cfg(feature = "gpu")]
    fn primeiro_lote(r: &mut dyn Rasterizador, (largura, altura): (usize, usize)) {
        r.set_viewport(0, 0, largura as i32, altura as i32);
        r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        r.clear(gles::GL_COLOR_BUFFER_BIT);
        triangulo(r, [1.0, 0.0, 0.0, 1.0], [(-1.0, 1.0), (-1.0, -1.0), (1.0, 1.0)]);
        // Fechar o lote é o que o põe na placa — e é ali que o contexto volta ao anfitrião.
        r.descarrega_o_desenho();
    }

    /// O segundo lote: outro triângulo, na outra metade do quadro.
    #[cfg(feature = "gpu")]
    fn segundo_lote(r: &mut dyn Rasterizador) {
        triangulo(r, [0.0, 1.0, 0.0, 1.0], [(1.0, -1.0), (1.0, 1.0), (-1.0, -1.0)]);
        r.descarrega_o_desenho();
    }

    /// **Um segundo desenhista no mesmo contexto não apaga o desenho do jogo.**
    ///
    /// É a metade que faltava do achado 2: nenhum teste punha o `Pintor` do `egui` entre dois lotes
    /// do rasterizador, **no mesmo contexto** — que é o caminho de `graphics.gpu_rasterizer = true`
    /// (`src/ui/app.rs` entrega o contexto do `eframe` à sessão, e `src/ui/gpu.rs` o pinta). O que
    /// o `Pintor` faz no fim de cada pintura está copiado aqui: `use_program(None)` e
    /// `bind_vertex_array(None)`. O quadro dos dois lados tem de sair **igual**.
    #[cfg(feature = "gpu")]
    #[test]
    fn o_pintor_no_mesmo_contexto_nao_apaga_o_desenho_do_jogo() {
        use glow::HasContext as _;

        let (largura, altura) = (16, 16);
        let Some((contexto, gl)) = placa_emprestada() else {
            return;
        };
        let (Some(mut so_o_primeiro), Some(mut referencia), Some(mut com_pintor)) = (
            estado_emprestado(largura, altura, &gl),
            estado_emprestado(largura, altura, &gl),
            estado_emprestado(largura, altura, &gl),
        ) else {
            return;
        };
        let _ = &contexto;
        let medida = (largura, altura);

        primeiro_lote(&mut so_o_primeiro, medida);
        primeiro_lote(&mut referencia, medida);
        primeiro_lote(&mut com_pintor, medida);

        // **O `Pintor` do `egui` pinta aqui**, no mesmo contexto, e é assim que ele termina.
        unsafe {
            gl.use_program(None);
            gl.bind_vertex_array(None);
        }

        // Na referência ninguém mexeu no contexto; no outro, o `egui` acabou de desligar os dois.
        segundo_lote(&mut referencia);
        segundo_lote(&mut com_pintor);

        let so_um = so_o_primeiro.read_rect(0, 0, largura, altura);
        let esperado = referencia.read_rect(0, 0, largura, altura);
        let obtido = com_pintor.read_rect(0, 0, largura, altura);
        assert_ne!(
            so_um, esperado,
            "a cena do teste não discrimina: o segundo lote não desenhou nada nem na referência"
        );
        assert_eq!(
            obtido, esperado,
            "o segundo lote desenhou com o programa que o egui deixou: o cache dos objetos ligados \
             sobreviveu a outra pessoa usar o contexto"
        );
    }

    /// **A placa que morreu no meio do quadro não recebe desenho novo.**
    ///
    /// O aviso de que o contexto acabou chega de fora do emulador e **não espera** o `retro_run`
    /// terminar (auditoria de GPU, achados 1 e 4): o que se cobra é que o rasterizador pare de
    /// chamar o driver assim que souber, em vez de desenhar por ponteiros de função que já não
    /// existem. O quadro que sai é o que já estava na placa.
    #[cfg(feature = "gpu")]
    #[test]
    fn a_placa_que_morreu_no_meio_do_quadro_nao_desenha_mais() {
        let (largura, altura) = (16, 16);
        let Some((contexto, gl)) = placa_emprestada() else {
            return;
        };
        // Os dois estados do teste nascem antes do aviso: criar um depois dele seria pedir GL a uma
        // placa morta. Os dois são do **mesmo** contexto — a marca de óbito é da placa, e não do
        // estado, que é o que o aviso do frontend quer dizer.
        let (Some(mut vivo), Some(mut com_a_morta)) = (
            estado_emprestado(largura, altura, &gl),
            estado_emprestado(largura, altura, &gl),
        ) else {
            return;
        };
        let _ = &contexto;
        let medida = (largura, altura);

        // **A cena discrimina:** com a placa viva, o segundo lote muda o quadro. É o controle deste
        // teste, e ele roda no mesmo contexto e na mesma medida do caso que interessa.
        primeiro_lote(&mut vivo, medida);
        let antes = vivo.read_rect(0, 0, largura, altura);
        segundo_lote(&mut vivo);
        let depois = vivo.read_rect(0, 0, largura, altura);
        assert_ne!(
            antes, depois,
            "a cena do teste não discrimina: com a placa viva o segundo lote tem de aparecer"
        );

        // O aviso, no meio do quadro — o mesmo que o `contexto_perdido` e o `contexto_reset` dão.
        primeiro_lote(&mut com_a_morta, medida);
        let antes = com_a_morta.read_rect(0, 0, largura, altura);
        a_placa_morreu(endereco_da_placa(&gl));
        segundo_lote(&mut com_a_morta);
        let depois = com_a_morta.read_rect(0, 0, largura, altura);
        assert_eq!(
            depois,
            antes,
            "a placa morta continuou desenhando: o estado não foi invalidado no aviso"
        );

        // A marca é do processo todo: limpa, senão o teste seguinte herdaria uma placa morta.
        a_placa_nasceu(endereco_da_placa(&gl));
    }
}

