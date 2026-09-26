//! A biblioteca andando pelo controle e pelo teclado, e não só pelo mouse.
//!
//! O direcional, o manche esquerdo e as setas escolhem; o botão 1, o Start, o Enter e o espaço
//! abrem; o HOME abre a Z-Wheel. Segurar uma direção repete. Saiu do egui para que a biblioteca Qt
//! ande igual: quem lê o teclado e desenha é cada janela, e o que vira comando é daqui.

use std::time::{Duration, Instant};

use crate::input::Pad;

/// Segurar uma direção repete a escolha depois desta espera, e neste ritmo.
const REPETE_DEPOIS: Duration = Duration::from_millis(380);
const REPETE_A_CADA: Duration = Duration::from_millis(110);
/// Quanto o manche precisa sair do centro para valer como direção, no curso de ±128.
const MANCHE: i32 = 72;

/// O que a entrada pede à biblioteca.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comando {
    Cima,
    Baixo,
    Esquerda,
    Direita,
    Abrir,
    ZWheel,
}

const COMANDOS: [Comando; 6] = [
    Comando::Cima,
    Comando::Baixo,
    Comando::Esquerda,
    Comando::Direita,
    Comando::Abrir,
    Comando::ZWheel,
];

/// O que está apertado agora, na ordem dos [`Comando`]: `[cima, baixo, esquerda, direita, abrir,
/// home]`.
///
/// `teclas` é o teclado da janela — as quatro setas e o abrir (Enter ou espaço) —, e `pads` os
/// controles das portas ligadas. **Os dois viram um estado só antes de detectar o aperto:** uma
/// seta mapeada também no controle continua sendo um passo, e não dois.
pub fn apertado(teclas: [bool; 5], pads: &[Pad]) -> [bool; 6] {
    let botao = |nome: &str| {
        Pad::button_by_name(nome).is_some_and(|i| pads.iter().any(|pad| pad.is_down(i)))
    };
    let eixo = |eixo: usize, sinal: i32| pads.iter().any(|pad| pad.axes[eixo] * sinal > MANCHE);
    [
        teclas[0] || botao("up") || eixo(1, -1),
        teclas[1] || botao("down") || eixo(1, 1),
        teclas[2] || botao("left") || eixo(0, -1),
        teclas[3] || botao("right") || eixo(0, 1),
        teclas[4] || botao("b1") || botao("start"),
        botao("back"),
    ]
}

/// Transforma o que está apertado em comandos: só o aperto conta, e a direção segurada repete.
#[derive(Default)]
pub struct Navegacao {
    /// O que estava apertado na leitura anterior.
    antes: [bool; 6],
    /// Desde quando a direção segurada está apertada, e quando ela repetiu pela última vez.
    segurando: Option<(usize, Instant, Instant)>,
}

impl Navegacao {
    /// A biblioteca não está escutando — um jogo aberto, as configurações por cima. Tudo conta
    /// como já apertado, para o botão que a fecha não abrir um jogo ao voltar.
    pub fn silencia(&mut self) {
        self.antes = [true; 6];
        self.segurando = None;
    }

    /// Os comandos desta leitura.
    pub fn comandos(&mut self, agora: [bool; 6], instante: Instant) -> Vec<Comando> {
        let mut comandos = Vec::new();
        for (i, comando) in COMANDOS.iter().enumerate() {
            if agora[i] && !self.antes[i] {
                comandos.push(*comando);
                if i < 4 {
                    self.segurando = Some((i, instante, instante));
                }
            }
        }
        match self.segurando {
            Some((i, _, _)) if !agora[i] => self.segurando = None,
            Some((i, desde, ultima))
                if instante - desde >= REPETE_DEPOIS && instante - ultima >= REPETE_A_CADA =>
            {
                comandos.push(COMANDOS[i]);
                self.segurando = Some((i, desde, instante));
            }
            _ => {}
        }
        self.antes = agora;
        comandos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIREITA: [bool; 6] = [false, false, false, true, false, false];
    const NADA: [bool; 6] = [false; 6];

    /// Só o aperto vira comando, e a direção segurada repete depois da espera, no ritmo.
    #[test]
    fn segurar_a_direcao_repete_no_ritmo() {
        let inicio = Instant::now();
        let mut navegacao = Navegacao::default();
        assert_eq!(navegacao.comandos(DIREITA, inicio), vec![Comando::Direita]);
        let cedo = inicio + Duration::from_millis(200);
        assert!(navegacao.comandos(DIREITA, cedo).is_empty(), "antes da espera");
        let depois = inicio + REPETE_DEPOIS;
        assert_eq!(navegacao.comandos(DIREITA, depois), vec![Comando::Direita]);
        let logo = depois + Duration::from_millis(50);
        assert!(navegacao.comandos(DIREITA, logo).is_empty(), "antes do ritmo");
        assert!(navegacao.comandos(NADA, logo).is_empty());
    }

    /// Silenciada, a biblioteca não toma como aperto o que já estava apertado.
    #[test]
    fn silenciar_nao_deixa_o_botao_de_fechar_abrir_um_jogo() {
        let mut navegacao = Navegacao::default();
        navegacao.silencia();
        let abrir = [false, false, false, false, true, false];
        assert!(navegacao.comandos(abrir, Instant::now()).is_empty());
    }

    /// A seta do teclado e o direcional do controle são um aperto só.
    #[test]
    fn teclado_e_controle_juntos_sao_um_passo() {
        let mut pad = Pad::default();
        pad.press(Pad::button_by_name("right").unwrap(), true);
        let agora = apertado([false, false, false, true, false], &[pad]);
        assert_eq!(agora, DIREITA);
        let mut manche = Pad::default();
        manche.axes[0] = 100;
        assert_eq!(apertado([false; 5], &[manche]), DIREITA, "o manche também anda");
    }
}
