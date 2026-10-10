//! O rewind: pontos de retorno marcados de tempos em tempos, e a volta de ponto em ponto.
//!
//! **Pontos, e não um rewind liso.** Um estado do jogo tem de 15 a 66 MB e leva de 5 a 37 ms para
//! ser gravado (medido em 2026-10-10 no rasterizador de processador: Double Dragon 14,8 MB, Crash
//! 14–19 MB, Zeebo Extreme Rolima 38–66 MB). Gravar a cada quadro, como o rewind do RetroArch
//! faz, não cabe em 16 ms. Um ponto a cada segundo de jogo custa um soluço desse tamanho uma vez
//! por segundo, e por isso o rewind vem desligado.
//!
//! **Só a cópia roda no laço do jogo.** A compressão — 26 ms no Double Dragon, 218 ms no Rolima
//! com o deflate rápido — vai para uma thread à parte, que guarda o ponto comprimido no anel. O
//! anel tem teto de memória, e não de quantidade: o mesmo teto guarda uns 75 pontos do Double
//! Dragon e uns 13 do Rolima. Ver `docs/implementacao/24-velocidade.md`.

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::session::Session;

/// O rewind como o usuário o escolheu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AjustesDoRewind {
    pub ligado: bool,
    /// De quanto em quanto tempo de jogo um ponto é marcado.
    pub intervalo_ms: u32,
    /// O teto do anel, em megabytes de pontos comprimidos.
    pub memoria_mb: u32,
}

impl AjustesDoRewind {
    pub const INTERVALOS_MS: [u32; 4] = [500, 1000, 2000, 5000];
    pub const MEMORIAS_MB: [u32; 5] = [64, 128, 256, 512, 1024];

    /// Metade no Android: um aparelho de 3 GB não tem os 256 MB do desktop para dar.
    #[cfg(target_os = "android")]
    pub const MEMORIA_PADRAO_MB: u32 = 128;
    #[cfg(not(target_os = "android"))]
    pub const MEMORIA_PADRAO_MB: u32 = 256;

    fn teto_em_bytes(&self) -> usize {
        self.memoria_mb.clamp(16, 4096) as usize * 1024 * 1024
    }
}

impl Default for AjustesDoRewind {
    fn default() -> Self {
        Self {
            ligado: false,
            intervalo_ms: 1000,
            memoria_mb: Self::MEMORIA_PADRAO_MB,
        }
    }
}

/// Um ponto de retorno: o relógio do jogo em que ele foi marcado e o estado comprimido.
struct Ponto {
    relogio_ms: u32,
    comprimido: Vec<u8>,
}

/// Os pontos, do mais velho ao mais novo, e quanto eles ocupam.
#[derive(Default)]
struct Anel {
    pontos: VecDeque<Ponto>,
    bytes: usize,
    teto: usize,
    /// Muda a cada [`Rewind::limpa`]: um ponto que a thread terminou de comprimir depois disso é
    /// de um jogo que já não existe, e não entra.
    geracao: u64,
}

impl Anel {
    fn guarda(&mut self, ponto: Ponto) {
        self.bytes += ponto.comprimido.len();
        self.pontos.push_back(ponto);
        // Sai o mais velho até caber. O mais novo fica mesmo sozinho acima do teto: um ponto só
        // é melhor que nenhum, e o teto é sobre o que se acumula.
        while self.bytes > self.teto && self.pontos.len() > 1 {
            if let Some(velho) = self.pontos.pop_front() {
                self.bytes -= velho.comprimido.len();
            }
        }
    }

    fn tira_o_mais_novo(&mut self) -> Option<Ponto> {
        let ponto = self.pontos.pop_back()?;
        self.bytes -= ponto.comprimido.len();
        Some(ponto)
    }
}

/// O que vai para a thread de compressão: a geração do anel, o relógio e o estado cru.
type Pedido = (u64, u32, Vec<u8>);

/// O rewind de uma partida.
pub struct Rewind {
    anel: Arc<Mutex<Anel>>,
    envio: Option<SyncSender<Pedido>>,
    /// A thread está comprimindo: um ponto novo agora seria gravado para ficar na fila, e a
    /// gravação é o que custa no laço do jogo. Melhor pular a marca e tentar na próxima volta.
    ocupada: Arc<AtomicBool>,
    trabalhadora: Option<std::thread::JoinHandle<()>>,
    ajustes: AjustesDoRewind,
    /// O relógio do jogo no último ponto marcado ou restaurado.
    ultimo_ms: Option<u32>,
}

impl Rewind {
    pub fn novo(ajustes: AjustesDoRewind) -> Self {
        let anel = Arc::new(Mutex::new(Anel {
            teto: ajustes.teto_em_bytes(),
            ..Anel::default()
        }));
        let ocupada = Arc::new(AtomicBool::new(false));
        // Um pedido na fila, no máximo: a thread comprime um enquanto o laço já pode marcar o
        // seguinte, e mais que isso seria memória crua (até 66 MB cada) esperando.
        let (envio, recebe) = std::sync::mpsc::sync_channel::<Pedido>(1);
        let trabalhadora = {
            let anel = Arc::clone(&anel);
            let ocupada = Arc::clone(&ocupada);
            std::thread::Builder::new()
                .name("zeebx-rewind".to_string())
                .spawn(move || comprime_para_sempre(recebe, anel, ocupada))
                .ok()
        };
        Self {
            anel,
            envio: trabalhadora.is_some().then_some(envio),
            ocupada,
            trabalhadora,
            ajustes,
            ultimo_ms: None,
        }
    }

    /// Troca os ajustes de um rewind que já existe: o intervalo vale no próximo ponto, e o teto
    /// novo despeja o que passar dele no próximo que entrar.
    pub fn ajusta(&mut self, ajustes: AjustesDoRewind) {
        self.ajustes = ajustes;
        if let Ok(mut anel) = self.anel.lock() {
            anel.teto = ajustes.teto_em_bytes();
        }
    }

    /// Marca um ponto se já passou o intervalo e a sessão deixa. Chamado depois de cada volta do
    /// jogo; não faz nada na maior parte delas.
    pub fn acompanha(&mut self, sessao: &mut Session) {
        let Some(envio) = &self.envio else {
            return;
        };
        let agora = sessao.clock_ms();
        // `wrapping_sub` porque o relógio de um ponto restaurado pode estar à frente do de agora
        // — e aí a diferença dá a volta e marca na hora, o que é o certo.
        let venceu = self
            .ultimo_ms
            .is_none_or(|ultimo| agora.wrapping_sub(ultimo) >= self.ajustes.intervalo_ms);
        if !venceu || self.ocupada.load(Ordering::Acquire) {
            return;
        }
        // Um desenho começado ou um arquivo aberto para escrita: tenta na próxima volta, sem
        // reiniciar o intervalo.
        if sessao.pode_marcar_ponto().is_err() {
            return;
        }
        let geracao = self.anel.lock().map(|anel| anel.geracao).unwrap_or_default();
        let estado = sessao.grava_estado();
        self.ocupada.store(true, Ordering::Release);
        match envio.try_send((geracao, agora, estado)) {
            Ok(()) => self.ultimo_ms = Some(agora),
            Err(TrySendError::Full(_)) => self.ocupada.store(false, Ordering::Release),
            Err(TrySendError::Disconnected(_)) => {
                self.ocupada.store(false, Ordering::Release);
                self.envio = None;
            }
        }
    }

    /// Volta ao ponto mais novo, e o tira do anel. `false` quando não há ponto nenhum.
    ///
    /// Um ponto que o motor recusa — um arquivo que o jogo apagou depois, por exemplo — sai do
    /// anel e o anterior é tentado: o jogador vê o rewind pular um ponto, e nada quebra.
    pub fn volta_um(&mut self, sessao: &mut Session) -> bool {
        loop {
            let Some(ponto) = self.anel.lock().ok().and_then(|mut anel| anel.tira_o_mais_novo()) else {
                return false;
            };
            let estado = match descomprime(&ponto.comprimido) {
                Ok(estado) => estado,
                Err(erro) => {
                    crate::registro!(
                        crate::registro::Nivel::Aviso,
                        "rewind",
                        "o ponto de {} ms não descomprimiu: {erro}",
                        ponto.relogio_ms
                    );
                    continue;
                }
            };
            match sessao.restaura_estado(&estado) {
                Ok(()) => {
                    self.ultimo_ms = Some(ponto.relogio_ms);
                    return true;
                }
                Err(erro) => crate::registro!(
                    crate::registro::Nivel::Aviso,
                    "rewind",
                    "o ponto de {} ms foi recusado e saiu do anel: {erro}",
                    ponto.relogio_ms
                ),
            }
        }
    }

    /// Esquece todos os pontos. É o que carregar um save state pede: os pontos são de outra
    /// linha do tempo.
    pub fn limpa(&mut self) {
        if let Ok(mut anel) = self.anel.lock() {
            anel.pontos.clear();
            anel.bytes = 0;
            anel.geracao = anel.geracao.wrapping_add(1);
        }
        self.ultimo_ms = None;
    }

    /// Quantos pontos há agora, e quanto eles ocupam.
    pub fn tamanho(&self) -> (usize, usize) {
        self.anel
            .lock()
            .map(|anel| (anel.pontos.len(), anel.bytes))
            .unwrap_or_default()
    }

    /// Espera a thread terminar o ponto em curso. Só para os testes, que precisam do ponto no
    /// anel antes de voltar a ele.
    #[cfg(test)]
    fn espera_a_compressao(&self) {
        while self.ocupada.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
}

impl Drop for Rewind {
    fn drop(&mut self) {
        // Fechar o canal termina o laço da thread; esperar por ela é esperar no máximo um ponto.
        self.envio = None;
        if let Some(trabalhadora) = self.trabalhadora.take() {
            let _ = trabalhadora.join();
        }
    }
}

/// De quanto em quanto tempo real o rewind segurado volta um ponto. Com um ponto por segundo de
/// jogo, é voltar quatro vezes mais rápido do que se jogou.
pub const PASSO_DO_REWIND: std::time::Duration = std::time::Duration::from_millis(250);

/// O rewind como uma janela o usa: o atalho segurado, o passo, o som calado e o relógio.
///
/// Mora aqui, e não em cada frontend, porque o desktop (pelo `ui::partida`) e o Android fazem a
/// mesma coisa com o mesmo atalho.
#[derive(Default)]
pub struct ControleDoRewind {
    rewind: Option<Rewind>,
    /// O atalho estava apertado na volta anterior.
    antes: bool,
    /// O jogo está voltando: não anda, e o fast-forward não vale.
    voltando: bool,
    /// Quando o próximo ponto pode ser restaurado, com o atalho segurado.
    proximo: Option<std::time::Instant>,
    /// O último pedido de voltar não achou ponto nenhum.
    sem_pontos: bool,
}

/// O que uma leitura do atalho pede a quem chama, além do que já foi feito na sessão.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leitura {
    /// Nada de novo.
    Nada,
    /// O atalho foi solto: o tempo real que passou voltando não é para o jogo correr atrás, e
    /// quem mede esse tempo do lado de fora (a fatia da janela) tem de recomeçar também.
    Soltou,
}

impl ControleDoRewind {
    /// Lê o atalho desta volta. Vem **antes** do fast-forward e do passo do jogo: voltando, o
    /// jogo não anda.
    ///
    /// Segurado, volta um ponto a cada [`PASSO_DO_REWIND`] e mostra o quadro dele, com o som
    /// calado; solto, o jogo segue do último ponto mostrado, e os mais novos já saíram do anel.
    /// Pausado, cada toque volta um ponto e o jogo continua pausado — é o jeito de achar o ponto
    /// certo antes de soltar.
    pub fn le(
        &mut self,
        sessao: &mut Session,
        apertado: bool,
        pausado: bool,
        ajustes: &AjustesDoRewind,
    ) -> Leitura {
        if !ajustes.ligado {
            // Desligar no meio solta o jogo e o som, e larga a memória do anel.
            let soltou = self.voltando;
            if soltou {
                sessao.cala(false);
                sessao.recomeca_o_relogio();
            }
            *self = Self::default();
            return match soltou {
                true => Leitura::Soltou,
                false => Leitura::Nada,
            };
        }
        let rewind = self.rewind.get_or_insert_with(|| Rewind::novo(*ajustes));
        rewind.ajusta(*ajustes);
        let desceu = apertado && !self.antes;
        let soltou = !apertado && self.antes;
        self.antes = apertado;
        let agora = std::time::Instant::now();
        let venceu = self.proximo.is_none_or(|proximo| agora >= proximo);
        if apertado && (desceu || (!pausado && venceu)) {
            self.sem_pontos = !rewind.volta_um(sessao);
            self.proximo = Some(agora + PASSO_DO_REWIND);
        }
        if apertado && !self.voltando {
            sessao.cala(true);
        }
        self.voltando = apertado;
        if !soltou {
            return Leitura::Nada;
        }
        self.sem_pontos = false;
        sessao.cala(false);
        sessao.recomeca_o_relogio();
        Leitura::Soltou
    }

    /// Marca um ponto, se for a hora. Chamado depois de cada volta em que o jogo andou.
    pub fn acompanha(&mut self, sessao: &mut Session) {
        if let Some(rewind) = &mut self.rewind {
            rewind.acompanha(sessao);
        }
    }

    pub fn voltando(&self) -> bool {
        self.voltando
    }

    /// O que a janela escreve por cima do jogo enquanto ele volta: `Some(true)` quando não há
    /// mais ponto nenhum.
    pub fn indicador(&self) -> Option<bool> {
        self.voltando.then_some(self.sem_pontos)
    }

    /// Esquece os pontos: um save state carregado é outra linha do tempo.
    pub fn esquece_os_pontos(&mut self) {
        if let Some(rewind) = &mut self.rewind {
            rewind.limpa();
        }
    }

    /// Esquece o atalho, sem largar os pontos: a entrada da janela recomeçou.
    pub fn solta(&mut self) {
        self.antes = false;
        self.voltando = false;
        self.sem_pontos = false;
    }
}

fn comprime_para_sempre(recebe: Receiver<Pedido>, anel: Arc<Mutex<Anel>>, ocupada: Arc<AtomicBool>) {
    while let Ok((geracao, relogio_ms, estado)) = recebe.recv() {
        let comprimido = comprime(&estado);
        drop(estado);
        if let (Ok(comprimido), Ok(mut anel)) = (comprimido, anel.lock())
            && anel.geracao == geracao
        {
            anel.guarda(Ponto {
                relogio_ms,
                comprimido,
            });
        }
        ocupada.store(false, Ordering::Release);
    }
}

fn comprime(estado: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut codificador =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    codificador.write_all(estado)?;
    codificador.finish()
}

fn descomprime(comprimido: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut estado = Vec::new();
    flate2::read::DeflateDecoder::new(comprimido).read_to_end(&mut estado)?;
    Ok(estado)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ponto(relogio_ms: u32, bytes: usize) -> Ponto {
        Ponto {
            relogio_ms,
            comprimido: vec![0; bytes],
        }
    }

    /// O teto despeja os mais velhos, e o mais novo fica mesmo sozinho acima dele.
    #[test]
    fn o_anel_respeita_o_teto_pelos_mais_velhos() {
        let mut anel = Anel {
            teto: 100,
            ..Anel::default()
        };
        anel.guarda(ponto(1, 40));
        anel.guarda(ponto(2, 40));
        anel.guarda(ponto(3, 40));
        let relogios: Vec<u32> = anel.pontos.iter().map(|p| p.relogio_ms).collect();
        assert_eq!(relogios, [2, 3]);
        assert_eq!(anel.bytes, 80);

        anel.guarda(ponto(4, 500));
        assert_eq!(anel.pontos.len(), 1, "o grande fica sozinho");
        assert_eq!(anel.tira_o_mais_novo().map(|p| p.relogio_ms), Some(4));
        assert_eq!(anel.bytes, 0);
        assert!(anel.tira_o_mais_novo().is_none());
    }

    #[test]
    fn comprimir_e_descomprimir_e_ida_e_volta() {
        let estado: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let comprimido = comprime(&estado).unwrap();
        assert!(comprimido.len() < estado.len());
        assert_eq!(descomprime(&comprimido).unwrap(), estado);
    }

    #[test]
    fn os_ajustes_de_fabrica() {
        let ajustes = AjustesDoRewind::default();
        assert!(!ajustes.ligado, "o rewind custa um soluço por ponto: vem desligado");
        assert_eq!(ajustes.intervalo_ms, 1000);
        assert!(AjustesDoRewind::MEMORIAS_MB.contains(&ajustes.memoria_mb));
        let lido: AjustesDoRewind = serde_json::from_str(r#"{"ligado": true}"#).unwrap();
        assert!(lido.ligado);
        assert_eq!(lido.intervalo_ms, 1000);
    }

    /// Com uma sessão de verdade: marca, a thread comprime, e voltar devolve o relógio do ponto.
    #[test]
    fn marcar_e_voltar_numa_sessao() {
        let mut sessao = crate::session::tests::sessao_minima_para_save_state();
        let mut rewind = Rewind::novo(AjustesDoRewind {
            ligado: true,
            ..AjustesDoRewind::default()
        });
        rewind.acompanha(&mut sessao);
        rewind.espera_a_compressao();
        assert_eq!(rewind.tamanho().0, 1);
        assert!(rewind.volta_um(&mut sessao));
        assert_eq!(rewind.tamanho().0, 0);
        assert!(!rewind.volta_um(&mut sessao), "sem pontos, não volta");

        // Limpar descarta também o que a thread ainda estiver comprimindo.
        rewind.ultimo_ms = None;
        rewind.acompanha(&mut sessao);
        rewind.limpa();
        rewind.espera_a_compressao();
        assert_eq!(rewind.tamanho().0, 0);
    }
}
