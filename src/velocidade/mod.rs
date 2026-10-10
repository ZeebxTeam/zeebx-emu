//! O ritmo do jogo contra o relógio do mundo: o limite de quadros e o pulo de desenho.
//!
//! O relógio do jogo é virtual (ver `docs/implementacao/04-tempo.md`): ele anda com o trabalho que
//! o jogo faz, e quem o prende ao relógio do mundo é a sessão, comparando os dois a cada volta.
//! Este módulo guarda **as decisões** dessa comparação — quanto o jogo pode correr e que quadros
//! deixam de ser desenhados —, separadas da sessão para valerem iguais em todo frontend e para
//! serem testadas sem jogo nenhum. O desenho geral está em
//! `docs/implementacao/24-velocidade.md`.

use serde::{Deserialize, Serialize};

pub mod rewind;
pub mod turbo;

/// O teto de velocidade do jogo.
///
/// **60 não quer dizer "chame o jogo 60 vezes por segundo"**: quer dizer "não deixe o relógio
/// virtual passar do relógio real", que é o freio que impede Crash, Zeebo Extreme e NFS de
/// correrem acima da velocidade do console. O tipo nasceu no core Libretro, e mora aqui para que
/// o desktop, o headless e o Android ofereçam as mesmas três escolhas com o mesmo significado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LimiteFps {
    /// Velocidade do console e apresentação normal.
    #[default]
    #[serde(rename = "60")]
    Sessenta,
    /// Velocidade do console, mas só um em cada dois quadros é desenhado.
    ///
    /// **Não desacelera a lógica**: o freio continua em 1x, e o quadro que não é desenhado deixa a
    /// tela com o anterior. Alterar o período virtual do vsync seria o oposto — o jogo avançaria
    /// 33 ms por quadro e poderia acelerar.
    #[serde(rename = "30")]
    Trinta,
    /// Sem freio: o jogo corre o quanto o host aguenta. É o boost de quem quer o NFS mais rápido.
    #[serde(rename = "desligado")]
    Desligado,
}

impl LimiteFps {
    pub const TODOS: [Self; 3] = [Self::Sessenta, Self::Trinta, Self::Desligado];

    /// Lê o texto de uma opção. Aceita o do core Libretro (`desligado`) e o do `config.ini` do
    /// headless (`off`), que é inglês porque quem o lê é o usuário estrangeiro. `None` para o
    /// que não é nenhum dos três, e quem chama mantém o que tinha.
    pub fn de_texto(texto: &str) -> Option<Self> {
        match texto.trim().to_ascii_lowercase().as_str() {
            "60" => Some(Self::Sessenta),
            "30" => Some(Self::Trinta),
            "desligado" | "off" => Some(Self::Desligado),
            _ => None,
        }
    }

    /// A chave do rótulo no catálogo de idiomas.
    pub fn chave(self) -> &'static str {
        match self {
            Self::Sessenta => "speed.fps_limit.60",
            Self::Trinta => "speed.fps_limit.30",
            Self::Desligado => "common.off",
        }
    }

    /// Se o relógio virtual é segurado contra o real.
    pub fn limita_velocidade(self) -> bool {
        !matches!(self, Self::Desligado)
    }
}

/// A política de pulo de desenho.
///
/// Pular **não muda a velocidade do jogo**: a lógica roda inteira, e só o desenho 3D e a limpeza
/// de tela daquele quadro deixam de acontecer — ver [`crate::machine::Machine::define_pula_desenho`].
/// O jogo que lê a tela de volta (`glReadPixels`) nunca pula: devolver a tela anterior a quem a
/// lê mudaria o que ele decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frameskip {
    #[default]
    Desligado,
    /// Pula `n` quadros a cada `n + 1` — `1` é metade, `2` é um terço, e por aí adiante.
    Fixo(u8),
    /// Pula só o que não ia à tela de qualquer jeito. No core Libretro é o aviso de buffer de
    /// áudio do frontend quem decide; fora dele, o atraso que a sessão já mede.
    Automatico,
}

impl Frameskip {
    /// O maior `n` do [`Frameskip::Fixo`]. Seis é o teto do core desde que a opção existe: um
    /// quadro desenhado a cada sete já é a imagem a 8 por segundo.
    pub const MAIOR_FIXO: u8 = 6;

    /// Todas as escolhas, na ordem dos menus: desligado, automático, e os fixos.
    pub fn todos() -> Vec<Self> {
        let mut todos = vec![Self::Desligado, Self::Automatico];
        todos.extend((1..=Self::MAIOR_FIXO).map(Self::Fixo));
        todos
    }

    /// Lê o texto de uma opção, como [`LimiteFps::de_texto`]: `desligado`/`off`,
    /// `automatico`/`auto`, ou um número de 1 a [`Frameskip::MAIOR_FIXO`].
    pub fn de_texto(texto: &str) -> Option<Self> {
        match texto.trim().to_ascii_lowercase().as_str() {
            "desligado" | "off" => Some(Self::Desligado),
            "automatico" | "auto" => Some(Self::Automatico),
            outro => outro
                .parse::<u8>()
                .ok()
                .filter(|n| (1..=Self::MAIOR_FIXO).contains(n))
                .map(Self::Fixo),
        }
    }

    /// O valor dentro da faixa: um `settings.json` editado à mão pode trazer `{"fixo": 40}`.
    pub fn normalizado(self) -> Self {
        match self {
            Self::Fixo(0) => Self::Desligado,
            Self::Fixo(n) => Self::Fixo(n.min(Self::MAIOR_FIXO)),
            outro => outro,
        }
    }
}

/// Como uma volta da sessão anda contra o relógio do mundo.
///
/// É o que o frontend entrega à sessão a cada volta, montado do que o usuário escolheu. Fica num
/// valor só porque as três coisas se cruzam: o limite decide a proporção e a meia apresentação, e
/// o pulo automático depende de a proporção existir.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ritmo {
    /// Quantas vezes a velocidade do console. `None` é sem freio.
    pub proporcao: Option<f32>,
    pub frameskip: Frameskip,
    /// Desenhar só um em cada dois quadros — o [`LimiteFps::Trinta`].
    pub meia_apresentacao: bool,
    /// Calar o som enquanto este ritmo vale: o "sem som" do fast-forward, e o sem limite dele.
    pub abafa_o_som: bool,
}

impl Ritmo {
    /// A velocidade do console, desenhando tudo: o que o `speed_limit` ligado sempre foi.
    pub const CONSOLE: Self = Self {
        proporcao: Some(1.0),
        frameskip: Frameskip::Desligado,
        meia_apresentacao: false,
        abafa_o_som: false,
    };

    /// Sem freio, desenhando tudo: o que o `speed_limit` desligado sempre foi.
    pub const SEM_FREIO: Self = Self {
        proporcao: None,
        frameskip: Frameskip::Desligado,
        meia_apresentacao: false,
        abafa_o_som: false,
    };

    /// O ritmo das escolhas do usuário.
    pub fn das_escolhas(limite: LimiteFps, frameskip: Frameskip) -> Self {
        Self {
            proporcao: limite.limita_velocidade().then_some(1.0),
            frameskip: frameskip.normalizado(),
            meia_apresentacao: limite == LimiteFps::Trinta,
            abafa_o_som: false,
        }
    }

    /// Este ritmo com o fast-forward por cima.
    ///
    /// A proporção passa a ser a do avanço, e o pulo de desenho fica **ao menos** automático: a
    /// 10x a volta roda dez quadros para mostrar um, e desenhar os nove que ninguém vê é o que
    /// impediria o host de chegar lá. Um pulo fixo escolhido continua valendo. A meia
    /// apresentação sai: o automático já pula mais do que ela.
    pub fn acelerado(self, avanco: &Avanco) -> Self {
        Self {
            proporcao: avanco.proporcao(),
            frameskip: match self.frameskip {
                Frameskip::Desligado => Frameskip::Automatico,
                outro => outro,
            },
            meia_apresentacao: false,
            abafa_o_som: avanco.abafa(),
        }
    }
}

/// Como um atalho de segurar se comporta: só enquanto apertado, ou um toque liga e outro desliga.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoDoAtalho {
    #[default]
    Segurar,
    Alternar,
}

impl ModoDoAtalho {
    pub const TODOS: [Self; 2] = [Self::Segurar, Self::Alternar];

    pub fn chave(self) -> &'static str {
        match self {
            Self::Segurar => "speed.shortcut_mode.hold",
            Self::Alternar => "speed.shortcut_mode.toggle",
        }
    }
}

/// O fast-forward, como o usuário o escolheu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Avanco {
    /// Quantas vezes a velocidade do console, de 2 a 10. **Zero é sem limite**: o quanto o host
    /// aguentar, sempre mudo.
    pub proporcao: u8,
    pub modo: ModoDoAtalho,
    /// Calar o som enquanto avança. Desligado, o som acelera junto com o jogo.
    pub sem_som: bool,
}

impl Default for Avanco {
    fn default() -> Self {
        Self {
            proporcao: 3,
            modo: ModoDoAtalho::Segurar,
            sem_som: false,
        }
    }
}

impl Avanco {
    /// A menor e a maior proporção oferecidas. Dez é o teto do RALibretro, que serviu de modelo.
    pub const MENOR: u8 = 2;
    pub const MAIOR: u8 = 10;

    /// As escolhas, na ordem dos menus: 2x a 10x, e o sem limite no fim.
    pub fn escolhas() -> Vec<u8> {
        let mut escolhas: Vec<u8> = (Self::MENOR..=Self::MAIOR).collect();
        escolhas.push(0);
        escolhas
    }

    /// A proporção para a sessão. `None` é sem limite.
    pub fn proporcao(&self) -> Option<f32> {
        match self.proporcao {
            0 => None,
            n => Some(f32::from(n.clamp(Self::MENOR, Self::MAIOR))),
        }
    }

    /// Se o som fica calado: por escolha, ou porque sem limite não há proporção que ele siga.
    ///
    /// **Sem limite é sempre mudo.** A velocidade real varia a cada instante — de quadro leve para
    /// quadro pesado —, e o som que a acompanhasse mudaria de altura o tempo todo.
    pub fn abafa(&self) -> bool {
        self.sem_som || self.proporcao == 0
    }
}

/// Um atalho que se segura ou se alterna, lido uma vez por volta.
///
/// Guarda o que é preciso para ver a **borda** — o instante em que o botão desceu —, que é o que
/// o modo de alternar conta. Segurar um atalho de alternar não liga e desliga a cada volta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Interruptor {
    ligado: bool,
    antes: bool,
}

impl Interruptor {
    /// Lê o botão desta volta e diz se a função está ativa.
    pub fn atualiza(&mut self, apertado: bool, modo: ModoDoAtalho) -> bool {
        let desceu = apertado && !self.antes;
        self.antes = apertado;
        match modo {
            ModoDoAtalho::Segurar => {
                self.ligado = false;
                apertado
            }
            ModoDoAtalho::Alternar => {
                if desceu {
                    self.ligado = !self.ligado;
                }
                self.ligado
            }
        }
    }

    /// Se o modo de alternar está ligado agora.
    pub fn ligado(&self) -> bool {
        self.ligado
    }

    /// Volta ao começo: desligado, e o botão tido como solto. Fechar o jogo não pode deixar um
    /// alternar ligado para o próximo.
    pub fn desliga(&mut self) {
        *self = Self::default();
    }
}

/// Que quadros deixam de ser desenhados, decidido **por quadro**.
///
/// Um contador que só andasse quando o usuário mexe na opção pularia — ou não — para sempre a
/// partir da primeira leitura, e não um quadro a cada `n + 1`. Por isso o estado mora aqui e
/// avança a cada [`ContadorDePulo::decide`], que quem chama faz uma vez por quadro, antes de ele
/// começar.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContadorDePulo {
    fixo: u32,
    metade: u32,
}

/// Por que um quadro vai sem desenho. Os dois motivos ficam separados porque o core Libretro
/// trata o da meia apresentação de outro jeito: ele **duplica** o quadro anterior para o
/// frontend, em vez de entregar uma tela que não mudou.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pulo {
    pub frameskip: bool,
    pub metade: bool,
}

impl Pulo {
    pub fn algum(self) -> bool {
        self.frameskip || self.metade
    }
}

impl ContadorDePulo {
    /// Decide o quadro que vai começar.
    ///
    /// `sobra` é o que o [`Frameskip::Automatico`] consulta: se este quadro **não** vai à tela de
    /// qualquer jeito — porque a volta ainda vai rodar outro depois dele, ou porque o frontend
    /// avisou que o áudio está para estourar. O pulo fixo e a meia apresentação não olham para
    /// isso: valem sempre.
    pub fn decide(&mut self, ritmo: &Ritmo, sobra: bool) -> Pulo {
        let frameskip = match ritmo.frameskip.normalizado() {
            Frameskip::Desligado => {
                self.fixo = 0;
                false
            }
            Frameskip::Fixo(n) => {
                let pula = self.fixo != 0;
                self.fixo = (self.fixo + 1) % (u32::from(n) + 1);
                pula
            }
            Frameskip::Automatico => sobra,
        };
        let metade = match ritmo.meia_apresentacao {
            true => {
                let pula = self.metade != 0;
                self.metade = (self.metade + 1) % 2;
                pula
            }
            false => {
                self.metade = 0;
                false
            }
        };
        Pulo { frameskip, metade }
    }
}

/// O que a janela escreve por cima do jogo durante o fast-forward.
///
/// `alvo` é a proporção pedida (`None` é sem limite) e `alcancada` a medida, em vezes a velocidade
/// do console. Quando o host não chega a 95% do pedido, a medida aparece ao lado, para quem
/// escolheu 10x saber que está em 4x; sem limite, a medida é o próprio rótulo.
pub fn rotulo_do_avanco(alvo: Option<f32>, alcancada: f32, virgula: bool) -> String {
    let vezes = |valor: f32| {
        let texto = match valor.fract().abs() < 0.05 {
            true => format!("{:.0}x", valor.round()),
            false => format!("{valor:.1}x"),
        };
        match virgula {
            true => texto.replace('.', ","),
            false => texto,
        }
    };
    match alvo {
        Some(alvo) if alcancada >= alvo * 0.95 => format!("▶▶ {}", vezes(alvo)),
        Some(alvo) => format!("▶▶ {} ({})", vezes(alvo), vezes(alcancada)),
        None => format!("▶▶ {}", vezes(alcancada)),
    }
}

/// Quanto o jogo está adiantado (positivo) ou atrasado (negativo), em milissegundos virtuais.
///
/// `jogo_ms` é o quanto o relógio virtual andou desde a âncora, e `real_ms` o quanto o relógio do
/// mundo andou. Com proporção 3, o jogo deveria ter andado três vezes o real; sem proporção não
/// há meta, e o jogo nunca está adiantado nem atrasado.
pub fn adiantamento_ms(jogo_ms: u64, real_ms: u64, proporcao: Option<f32>) -> i64 {
    let Some(proporcao) = proporcao else {
        return 0;
    };
    let meta = (real_ms as f64 * f64::from(proporcao)).round() as i64;
    jogo_ms as i64 - meta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_texto_das_opcoes_vira_o_valor_certo() {
        assert_eq!(LimiteFps::de_texto("60"), Some(LimiteFps::Sessenta));
        assert_eq!(LimiteFps::de_texto(" 30 "), Some(LimiteFps::Trinta));
        assert_eq!(LimiteFps::de_texto("desligado"), Some(LimiteFps::Desligado));
        assert_eq!(LimiteFps::de_texto("OFF"), Some(LimiteFps::Desligado));
        assert_eq!(LimiteFps::de_texto("120"), None);

        assert_eq!(Frameskip::de_texto("desligado"), Some(Frameskip::Desligado));
        assert_eq!(Frameskip::de_texto("off"), Some(Frameskip::Desligado));
        assert_eq!(Frameskip::de_texto("automatico"), Some(Frameskip::Automatico));
        assert_eq!(Frameskip::de_texto("auto"), Some(Frameskip::Automatico));
        assert_eq!(Frameskip::de_texto("1"), Some(Frameskip::Fixo(1)));
        assert_eq!(Frameskip::de_texto("6"), Some(Frameskip::Fixo(6)));
        assert_eq!(Frameskip::de_texto("0"), None);
        assert_eq!(Frameskip::de_texto("7"), None);
        assert_eq!(Frameskip::de_texto("4294967295"), None);
        assert_eq!(Frameskip::de_texto(""), None);
    }

    /// O `settings.json` guarda os valores com nomes que alguém consegue ler e editar.
    #[test]
    fn o_arquivo_guarda_os_valores_pelo_nome() {
        assert_eq!(serde_json::to_string(&LimiteFps::Trinta).unwrap(), r#""30""#);
        assert_eq!(serde_json::to_string(&Frameskip::Fixo(2)).unwrap(), r#"{"fixo":2}"#);
        assert_eq!(serde_json::to_string(&Frameskip::Automatico).unwrap(), r#""automatico""#);
        let lido: Frameskip = serde_json::from_str(r#"{"fixo":40}"#).unwrap();
        assert_eq!(lido.normalizado(), Frameskip::Fixo(Frameskip::MAIOR_FIXO));
        let zero: Frameskip = serde_json::from_str(r#"{"fixo":0}"#).unwrap();
        assert_eq!(zero.normalizado(), Frameskip::Desligado);
    }

    #[test]
    fn o_ritmo_das_escolhas() {
        let r = Ritmo::das_escolhas(LimiteFps::Sessenta, Frameskip::Desligado);
        assert_eq!(r, Ritmo::CONSOLE);
        let r = Ritmo::das_escolhas(LimiteFps::Desligado, Frameskip::Desligado);
        assert_eq!(r, Ritmo::SEM_FREIO);
        let r = Ritmo::das_escolhas(LimiteFps::Trinta, Frameskip::Fixo(2));
        assert_eq!(r.proporcao, Some(1.0));
        assert!(r.meia_apresentacao);
        assert_eq!(r.frameskip, Frameskip::Fixo(2));
    }

    #[test]
    fn o_avanco_por_cima_do_ritmo() {
        let avanco = Avanco::default();
        let r = Ritmo::das_escolhas(LimiteFps::Trinta, Frameskip::Desligado).acelerado(&avanco);
        assert_eq!(r.proporcao, Some(3.0));
        assert_eq!(r.frameskip, Frameskip::Automatico, "o avanço pula ao menos o que sobra");
        assert!(!r.meia_apresentacao);
        assert!(!r.abafa_o_som, "o som acelera junto por padrão");

        let fixo = Ritmo::das_escolhas(LimiteFps::Sessenta, Frameskip::Fixo(3));
        assert_eq!(fixo.acelerado(&avanco).frameskip, Frameskip::Fixo(3));

        let sem_limite = Avanco {
            proporcao: 0,
            ..Avanco::default()
        };
        let r = Ritmo::CONSOLE.acelerado(&sem_limite);
        assert_eq!(r.proporcao, None);
        assert!(r.abafa_o_som, "sem limite é sempre mudo");

        let mudo = Avanco {
            sem_som: true,
            ..Avanco::default()
        };
        assert!(Ritmo::CONSOLE.acelerado(&mudo).abafa_o_som);
    }

    #[test]
    fn as_proporcoes_do_avanco() {
        assert_eq!(Avanco::escolhas(), [2, 3, 4, 5, 6, 7, 8, 9, 10, 0]);
        let com = |proporcao| Avanco {
            proporcao,
            ..Avanco::default()
        };
        assert_eq!(com(10).proporcao(), Some(10.0));
        // Um arquivo editado à mão não passa da faixa.
        assert_eq!(com(40).proporcao(), Some(10.0));
        assert_eq!(com(1).proporcao(), Some(2.0));
        assert_eq!(com(0).proporcao(), None);
    }

    /// Segurar vale enquanto apertado; alternar troca só na descida, e segurar não pisca.
    #[test]
    fn o_interruptor_segura_e_alterna() {
        let mut i = Interruptor::default();
        let segura: Vec<bool> = [false, true, true, false]
            .into_iter()
            .map(|b| i.atualiza(b, ModoDoAtalho::Segurar))
            .collect();
        assert_eq!(segura, [false, true, true, false]);

        let mut i = Interruptor::default();
        let alterna: Vec<bool> = [true, true, true, false, false, true, false]
            .into_iter()
            .map(|b| i.atualiza(b, ModoDoAtalho::Alternar))
            .collect();
        assert_eq!(alterna, [true, true, true, true, true, false, false]);

        i.atualiza(true, ModoDoAtalho::Alternar);
        assert!(i.ligado());
        i.desliga();
        assert!(!i.ligado());
    }

    /// Fixo `n` desenha um quadro e pula `n`, e recomeça: a decisão é por quadro.
    #[test]
    fn o_pulo_fixo_desenha_um_a_cada_n_mais_um() {
        let ritmo = Ritmo {
            frameskip: Frameskip::Fixo(2),
            ..Ritmo::CONSOLE
        };
        let mut contador = ContadorDePulo::default();
        let pulos: Vec<bool> = (0..7).map(|_| contador.decide(&ritmo, false).frameskip).collect();
        assert_eq!(pulos, [false, true, true, false, true, true, false]);
    }

    /// Os 30 FPS desenham um sim, um não — e o pulo é do motivo "metade", que o core duplica.
    #[test]
    fn a_meia_apresentacao_alterna() {
        let ritmo = Ritmo::das_escolhas(LimiteFps::Trinta, Frameskip::Desligado);
        let mut contador = ContadorDePulo::default();
        let pulos: Vec<Pulo> = (0..4).map(|_| contador.decide(&ritmo, true)).collect();
        assert_eq!(
            pulos.iter().map(|p| p.metade).collect::<Vec<_>>(),
            [false, true, false, true]
        );
        assert!(pulos.iter().all(|p| !p.frameskip));
    }

    /// O automático só pula o que sobra, e desligar zera o contador do fixo.
    #[test]
    fn o_automatico_segue_a_sobra_e_desligar_zera() {
        let auto = Ritmo {
            frameskip: Frameskip::Automatico,
            ..Ritmo::CONSOLE
        };
        let mut contador = ContadorDePulo::default();
        assert!(contador.decide(&auto, true).frameskip);
        assert!(!contador.decide(&auto, false).frameskip);

        let fixo = Ritmo {
            frameskip: Frameskip::Fixo(1),
            ..Ritmo::CONSOLE
        };
        contador.decide(&fixo, false);
        contador.decide(&Ritmo::CONSOLE, false);
        // Voltou a desenhar o primeiro: o contador recomeçou.
        assert!(!contador.decide(&fixo, false).frameskip);
    }

    #[test]
    fn o_rotulo_do_avanco() {
        assert_eq!(rotulo_do_avanco(Some(3.0), 2.98, true), "▶▶ 3x");
        assert_eq!(rotulo_do_avanco(Some(10.0), 4.2, true), "▶▶ 10x (4,2x)");
        assert_eq!(rotulo_do_avanco(Some(10.0), 4.2, false), "▶▶ 10x (4.2x)");
        assert_eq!(rotulo_do_avanco(None, 6.0, true), "▶▶ 6x");
    }

    #[test]
    fn o_adiantamento_respeita_a_proporcao() {
        // Um segundo real, um segundo de jogo: em dia.
        assert_eq!(adiantamento_ms(1000, 1000, Some(1.0)), 0);
        // A 3x, um segundo real pede três de jogo: com dois, está atrasado um.
        assert_eq!(adiantamento_ms(2000, 1000, Some(3.0)), -1000);
        // A 3x, quatro de jogo em um real é um adiantado.
        assert_eq!(adiantamento_ms(4000, 1000, Some(3.0)), 1000);
        // Sem freio não há meta.
        assert_eq!(adiantamento_ms(9000, 1, None), 0);
    }
}
