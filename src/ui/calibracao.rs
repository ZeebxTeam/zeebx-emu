//! O aviso de calibração do Boomerang: o canto da janela do jogo que mostra o controle inclinando
//! enquanto o jogo calibra o movimento — ver `docs/implementacao/20-boomerang-e-wii-remote.md`.
//!
//! Saiu do `App` do egui para a janela Qt mostrar o mesmo aviso, na mesma hora. Aqui fica quando
//! ele abre, se o controle está parado, quando ele conclui e some, e quanto o modelo gira; o
//! desenho é de cada janela.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// O aviso aberto: as leituras recentes e em que ponto ele está.
struct Aviso {
    aberto_em: Instant,
    /// As últimas leituras, para dizer se o controle está parado.
    recentes: VecDeque<[f32; 3]>,
    parado_desde: Option<Instant>,
    concluido_em: Option<Instant>,
}

impl Aviso {
    /// Quanto a leitura pode variar e o controle ainda contar como parado, em g.
    const TOLERANCIA: f32 = 0.05;
    const LEITURAS: usize = 20;
    /// A animação até ficar reto, e quanto o aviso fica depois dela.
    const ASSENTA: Duration = Duration::from_millis(400);
    const FICA: Duration = Duration::from_millis(1200);
    /// Um aviso que nunca conclui não fica para sempre na tela.
    const MAXIMO: Duration = Duration::from_secs(30);

    fn novo(agora: Instant) -> Self {
        Self {
            aberto_em: agora,
            recentes: VecDeque::new(),
            parado_desde: None,
            concluido_em: None,
        }
    }

    /// Guarda uma leitura e diz se o controle está parado.
    fn amostra(&mut self, leitura: [f32; 3], agora: Instant) -> bool {
        if self.recentes.len() == Self::LEITURAS {
            self.recentes.pop_front();
        }
        self.recentes.push_back(leitura);
        let parado = self.recentes.len() == Self::LEITURAS
            && (0..3).all(|eixo| {
                let (menor, maior) = self.recentes.iter().fold((f32::MAX, f32::MIN), |(a, b), l| {
                    (a.min(l[eixo]), b.max(l[eixo]))
                });
                maior - menor < Self::TOLERANCIA
            });
        match (parado, self.parado_desde) {
            (true, None) => self.parado_desde = Some(agora),
            (false, _) => self.parado_desde = None,
            _ => {}
        }
        parado
    }

    fn conclui(&mut self, agora: Instant) {
        self.concluido_em.get_or_insert(agora);
    }

    fn progresso_da_conclusao(&self, agora: Instant) -> f32 {
        self.concluido_em.map_or(0.0, |em| {
            ((agora - em).as_secs_f32() / Self::ASSENTA.as_secs_f32()).clamp(0.0, 1.0)
        })
    }

    fn expirou(&self, agora: Instant) -> bool {
        agora - self.aberto_em > Self::MAXIMO
            || self
                .concluido_em
                .is_some_and(|em| agora - em > Self::ASSENTA + Self::FICA)
    }
}

/// O que o sensor da porta está fazendo, para a linha de baixo do aviso.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Situacao {
    /// A porta não tem sensor lendo: a janela diz qual e por quê.
    SemSensor,
    Parado,
    Mexendo,
}

/// O aviso como a janela o desenha neste quadro.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vista {
    /// O jogo terminou de calibrar: o título muda e a linha de baixo some.
    pub concluido: bool,
    /// `None` quando concluído.
    pub situacao: Option<Situacao>,
    /// Quanto o modelo do Boomerang gira, em radianos: a inclinação do volante, que vai a zero
    /// numa animação curta quando a calibração conclui — o modelo "assenta".
    pub giro: f32,
}

/// O acompanhamento das calibrações de uma partida.
#[derive(Default)]
pub struct Calibracao {
    aviso: Option<Aviso>,
    /// As contagens de calibração começadas e terminadas já vistas. São da sessão: um jogo novo
    /// começa do zero.
    vistas: (u32, u32),
}

impl Calibracao {
    /// Um jogo novo abriu.
    pub fn reinicia(&mut self) {
        *self = Self::default();
    }

    /// O aviso neste quadro, ou `None` se não há aviso a mostrar.
    ///
    /// `calibracao` é a contagem da sessão, [`crate::session::Session::calibracao`]. `leitura` é o
    /// movimento da porta de Boomerang e se o sensor dela está lendo — `None` quando nenhuma porta
    /// ligada é de Boomerang. `ligado` é a opção `movimento.aviso_de_calibracao`.
    pub fn quadro(
        &mut self,
        calibracao: (u32, u32),
        leitura: Option<([f32; 3], bool)>,
        ligado: bool,
        agora: Instant,
    ) -> Option<Vista> {
        let (comecadas, terminadas) = calibracao;
        let novas = (comecadas > self.vistas.0, terminadas > self.vistas.1);
        self.vistas = calibracao;
        let Some((movimento, com_sensor)) = leitura else {
            self.aviso = None;
            return None;
        };
        if novas.0 && ligado {
            self.aviso = Some(Aviso::novo(agora));
        }
        let aviso = self.aviso.as_mut()?;
        if novas.1 {
            aviso.conclui(agora);
        }
        // Parado é só informação: quem diz que calibrou é o jogo. Concluir por estar parado dizia
        // "calibrado" enquanto o Crash Nitro Kart ainda recusava as leituras.
        let parado = aviso.amostra(movimento, agora);
        if aviso.expirou(agora) {
            self.aviso = None;
            return None;
        }
        let [x, y, _] = movimento;
        let no_plano = (x * x + y * y).sqrt();
        let volante = x.atan2(y) * ((no_plano - 0.3) / 0.4).clamp(0.0, 1.0);
        let concluido = aviso.concluido_em.is_some();
        let situacao = match (com_sensor, parado) {
            _ if concluido => None,
            (false, _) => Some(Situacao::SemSensor),
            (true, true) => Some(Situacao::Parado),
            (true, false) => Some(Situacao::Mexendo),
        };
        Some(Vista {
            concluido,
            situacao,
            giro: volante * (1.0 - aviso.progresso_da_conclusao(agora)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPOUSO: [f32; 3] = [0.0, 0.0, 1.0];

    /// O aviso só abre quando o jogo começa uma calibração, e só com a opção ligada.
    #[test]
    fn o_aviso_abre_quando_a_calibracao_comeca() {
        let agora = Instant::now();
        let mut calibracao = Calibracao::default();
        assert_eq!(calibracao.quadro((0, 0), Some((REPOUSO, true)), true, agora), None);
        assert!(calibracao.quadro((1, 0), Some((REPOUSO, true)), true, agora).is_some());

        let mut desligado = Calibracao::default();
        assert_eq!(desligado.quadro((1, 0), Some((REPOUSO, true)), false, agora), None);
    }

    /// Parado só depois de vinte leituras iguais; concluído é o jogo quem diz, e o aviso some
    /// depois de assentar e ficar.
    #[test]
    fn parado_e_concluido_sao_coisas_diferentes() {
        let inicio = Instant::now();
        let mut calibracao = Calibracao::default();
        let mut vista = None;
        for i in 0..Aviso::LEITURAS {
            let agora = inicio + Duration::from_millis(i as u64 * 10);
            vista = calibracao.quadro((1, 0), Some((REPOUSO, true)), true, agora);
        }
        let vista = vista.expect("o aviso está aberto");
        assert_eq!(vista.situacao, Some(Situacao::Parado));
        assert!(!vista.concluido, "parado não é calibrado");

        let concluiu = inicio + Duration::from_secs(1);
        let vista = calibracao.quadro((1, 1), Some((REPOUSO, true)), true, concluiu);
        assert_eq!(vista.map(|v| (v.concluido, v.situacao)), Some((true, None)));
        let depois = concluiu + Aviso::ASSENTA + Aviso::FICA + Duration::from_millis(1);
        assert_eq!(calibracao.quadro((1, 1), Some((REPOUSO, true)), true, depois), None);
    }

    /// Sem porta de Boomerang não há aviso, mesmo com calibração correndo.
    #[test]
    fn sem_boomerang_nao_ha_aviso() {
        let mut calibracao = Calibracao::default();
        assert_eq!(calibracao.quadro((1, 0), None, true, Instant::now()), None);
    }
}
