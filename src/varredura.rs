//! Varredura de uma ROM de verdade, dentro do `cargo test`.
//!
//! O relatório do `run` já diz o que precisamos saber de um jogo: onde parou, o que pediu e não
//! temos, quanto custou. O que faltava era **cobrar** isso. Uma varredura feita à mão, num laço
//! de shell, descobre a regressão na próxima vez que alguém a repetir — que pode ser semanas
//! depois do commit que a causou. Aqui o mesmo levantamento vira teste: a ROM avança, o
//! relatório sai estruturado, e o resumo pode ser comparado com o que já se sabia dela.
//!
//! Nenhuma ROM está na árvore, então **nada aqui roda sozinho**: os testes só fazem trabalho
//! quando `ZEEBX_ROM` aponta para um jogo, e sem isso avisam e passam — que é o certo para quem
//! clonou o repositório sem ter os arquivos. É também o que evita transformar `cargo test` numa
//! varredura de sessenta e cinco jogos sem ninguém ter pedido.
//!
//! ```bash
//! # um jogo, com o relatório inteiro na tela
//! ZEEBX_ROM="roms/Quake.zip" cargo test --release varredura -- --nocapture
//!
//! # a varredura inteira, gravando um relatório por jogo e cobrando a linha de base
//! ZEEBX_ROM=roms ZEEBX_ROM_MS=6000 ZEEBX_ROM_SAIDA=saida ZEEBX_ROM_BASE=docs/varredura \
//!   cargo test --release varredura -- --nocapture
//! ```
//!
//! | Variável | O que faz |
//! |---|---|
//! | `ZEEBX_ROM` | um `.mod`/`.zip`, uma lista separada por vírgula, ou um diretório |
//! | `ZEEBX_ROM_MS` | quanto tempo **virtual** rodar, em ms (padrão 6000, como o levantamento) |
//! | `ZEEBX_ROM_TETO` | teto de tempo **real** por jogo, em segundos (padrão 90) |
//! | `ZEEBX_ROM_SAIDA` | diretório onde gravar o relatório completo de cada jogo |
//! | `ZEEBX_ROM_BASE` | diretório da linha de base; o que não existe é gravado, o que existe é cobrado |
//!
//! O `--release` não é enfeite: em depuração o núcleo emulado roda uma ordem de grandeza mais
//! devagar, e o teto de tempo real classificaria jogo bom como "lento demais".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::machine::Outcome;
use crate::session::{Session, StartError, Step};

/// Quanto tempo virtual rodar quando ninguém pede outro. Seis segundos é o que o levantamento
/// de compatibilidade usa, e manter o mesmo número deixa os dois comparáveis.
const MS_PADRAO: u32 = 6_000;

/// Teto de tempo real por jogo. Quem não cumprir o tempo virtual dentro dele é "lento demais",
/// que é uma categoria de compatibilidade e não uma falha do teste — o mesmo critério do
/// `timeout 90` da varredura à mão.
const TETO_PADRAO: u64 = 90;

/// Quantos métodos mais chamados o relatório mostra. É onde gargalo aparece.
const CHAMADAS_MOSTRADAS: usize = 12;

/// Quantas linhas do log do próprio jogo entram no relatório. As últimas, não as primeiras: o
/// que interessa num jogo que quebrou é o que ele dizia pouco antes.
const LOG_MOSTRADO: usize = 20;

/// Em que estado a ROM ficou, nos mesmos baldes do levantamento de compatibilidade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Categoria {
    /// Não é um módulo que saibamos ler ou carregar. Nem chegou a haver máquina.
    NaoCarrega,
    /// O módulo carregou e a execução quebrou antes de haver applet: no `AEEMod_Load` ou
    /// dentro do `CreateInstance`. É onde para um arquivo que passa pelo cabeçalho frouxo do
    /// `.mod` sem ser código ARM.
    QuebrouAntesDoApplet,
    /// O módulo carregou e a execução voltou inteira, mas não há applet — nenhum `.mif` dizendo
    /// qual criar, ou `CreateInstance` recusando a classe.
    SemApplet,
    /// O applet nasceu e quebrou na primeira volta, que é onde o `EVT_APP_START` é entregue.
    QuebrouNaPartida,
    /// Quebrou já dentro do laço de quadros.
    QuebrouNoLaco,
    /// Não quebrou, mas não cumpriu o tempo virtual pedido dentro do teto de tempo real.
    LentoDemais,
    /// O jogo terminou sozinho antes do tempo pedido — sem timer armado e sem trabalho.
    Terminou,
    /// Cumpriu o tempo virtual pedido, de pé.
    Roda,
}

impl Categoria {
    /// Se este estado conta como teste passando.
    ///
    /// "Roda" e "terminou" passam; o resto é falha. **Pendência não é falha**: um jogo que roda
    /// pedindo três classes que não temos continua passando, porque é assim que ele se comporta
    /// no console também — o que cobra pendência é a linha de base, e só quando ela existe.
    pub fn passa(self) -> bool {
        matches!(self, Self::Roda | Self::Terminou)
    }

    fn rotulo(self) -> &'static str {
        match self {
            Self::NaoCarrega => "não carrega",
            Self::QuebrouAntesDoApplet => "quebrou antes de criar o applet",
            Self::SemApplet => "não cria o applet",
            Self::QuebrouNaPartida => "quebrou no EVT_APP_START",
            Self::QuebrouNoLaco => "quebrou no laço de quadros",
            Self::LentoDemais => "lento demais",
            Self::Terminou => "terminou sozinho",
            Self::Roda => "roda",
        }
    }
}

/// Quanto custou rodar. Nada daqui entra na comparação com a linha de base: são números da
/// máquina de quem rodou, e cobrá-los faria o teste falhar por causa do hardware.
#[derive(Debug, Clone, Copy, Default)]
pub struct Desempenho {
    /// Tempo real gasto de `start` até o applet existir. Um jogo pode levar segundos aqui.
    pub abertura: Duration,
    /// Tempo real gasto no laço de quadros.
    pub laco: Duration,
    /// Tempo virtual que o jogo chegou a medir, em ms.
    pub virtual_ms: u32,
    /// Voltas do laço de eventos. É o "parou na volta N" do levantamento.
    pub voltas: u64,
    /// Quadros que o jogo apresentou.
    pub quadros: u32,
    /// Instruções ARM executadas.
    pub instrucoes: u64,
    /// Chamadas de API atendidas.
    pub chamadas: u64,
    /// Bytes de heap do guest entregues, e objetos nossos vivos no fim.
    pub heap: u32,
    pub objetos: usize,
}

impl Desempenho {
    /// Fração da velocidade do console, em porcentagem: tempo virtual sobre tempo real.
    pub fn velocidade(&self) -> u64 {
        let real = self.laco.as_millis() as u64;
        match real {
            0 => 0,
            real => u64::from(self.virtual_ms) * 100 / real,
        }
    }

    /// Instruções por segundo de tempo real, no laço.
    pub fn ips(&self) -> u64 {
        match self.laco.as_millis() as u64 {
            0 => 0,
            real => self.instrucoes * 1000 / real,
        }
    }

    /// Quadros por segundo de tempo **virtual** — a taxa que o jogo acha que está tendo.
    pub fn fps(&self) -> u64 {
        match self.virtual_ms {
            0 => 0,
            ms => u64::from(self.quadros) * 1000 / u64::from(ms),
        }
    }
}

/// O que o emulador tem a apontar sobre a execução. É esta parte que a linha de base cobra.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pendencias {
    /// Métodos de API que o jogo chamou e não implementamos, com o endereço de quem chamou.
    pub apis: Vec<String>,
    /// ClassIDs que o jogo pediu e não temos.
    pub classes: Vec<String>,
    /// Arquivos que o jogo abriu e não achou. **Sinal, não ruído**: foi esta lista que entregou
    /// o `font.fnz` dos ports de arcade e os doze estágios do Resident Evil 4.
    pub arquivos: Vec<String>,
    /// Ponteiros do guest que recusamos, com o motivo.
    pub ponteiros: Vec<String>,
    /// APIs atendidas por hipótese — o que é palpite medido, e não comportamento conhecido.
    pub hipoteses: Vec<String>,
    /// Acessos inválidos que aconteceram dentro de um callback e não derrubaram a execução.
    pub falhas: Vec<String>,
    /// Texto que o jogo mandou desenhar e não soubemos desenhar.
    pub texto: Vec<String>,
    /// Chamadas de GL atendidas sem fazer nada.
    pub gl_ignorado: Vec<String>,
}

impl Pendencias {
    fn de(session: &Session) -> Self {
        let machine = session.machine();
        let ordenar = |mut linhas: Vec<String>| {
            linhas.sort();
            linhas.dedup();
            linhas
        };
        Self {
            apis: ordenar(machine.missing_apis()),
            classes: ordenar(
                machine
                    .unknown_classes()
                    .iter()
                    .map(|id| format!("{id:#010x}"))
                    .collect(),
            ),
            arquivos: ordenar(machine.missing_files()),
            ponteiros: ordenar(machine.bad_pointers()),
            hipoteses: ordenar(machine.assumptions().iter().map(|s| s.to_string()).collect()),
            falhas: ordenar(machine.swallowed_faults()),
            texto: ordenar(machine.pending_text().to_vec()),
            gl_ignorado: ordenar(machine.ignored_gl().iter().map(|s| s.to_string()).collect()),
        }
    }

    fn vazias(&self) -> bool {
        *self == Self::default()
    }

    /// As seções, na ordem em que valem a pena ser lidas — a mesma do relatório do `run`.
    fn secoes(&self) -> [(&'static str, &Vec<String>); 8] {
        [
            ("APIs que faltaram", &self.apis),
            ("classes que o jogo pediu e não temos", &self.classes),
            ("arquivos não encontrados", &self.arquivos),
            ("acessos inválidos que o jogo seguiu por cima", &self.falhas),
            ("ponteiros recusados", &self.ponteiros),
            ("APIs atendidas por hipótese", &self.hipoteses),
            ("texto que não soubemos desenhar", &self.texto),
            ("GL atendido sem fazer nada", &self.gl_ignorado),
        ]
    }
}

/// O levantamento de uma ROM.
#[derive(Debug, Clone)]
pub struct Relatorio {
    pub arquivo: PathBuf,
    pub titulo: String,
    pub categoria: Categoria,
    /// Onde parou, em texto legível. `None` quando o jogo seguia de pé no fim.
    pub motivo: Option<String>,
    /// `None` quando nem chegou a existir sessão para medir.
    pub desempenho: Option<Desempenho>,
    pub pendencias: Pendencias,
    /// Registradores e pilha no instante da falha, quando houve uma.
    ///
    /// Sem eles, "acesso inválido a 0x00000000" diz que alguma coisa era nula e não diz **qual**
    /// — e essa é a pergunta. Fica fora do resumo: o endereço e o `pc` já estão no motivo, e o
    /// resto é material de investigação, não de comparação.
    pub estado_da_falha: Option<String>,
    /// Os métodos mais chamados, do maior para o menor.
    pub chamadas_maiores: Vec<(String, u64)>,
    /// O fim do log do próprio jogo, por `DBGPRINTF` e por semihosting.
    pub log_do_jogo: Vec<String>,
}

impl Relatorio {
    /// O que a linha de base guarda: estado e pendências, sem número de desempenho.
    ///
    /// Tem de ser estável entre máquinas e entre execuções, ou a linha de base acusa regressão
    /// onde só houve um computador diferente. Por isso nada de tempo, de contagem de instrução
    /// nem de quadros aqui — só o que é comportamento do emulador diante daquele jogo.
    pub fn resumo(&self) -> String {
        let mut texto = format!(
            "{}\nestado: {}\n",
            self.titulo,
            self.categoria.rotulo()
        );
        if let Some(motivo) = &self.motivo {
            texto.push_str(&format!("motivo: {motivo}\n"));
        }
        for (nome, linhas) in self.pendencias.secoes() {
            if linhas.is_empty() {
                continue;
            }
            texto.push_str(&format!("{nome}:\n"));
            for linha in linhas {
                texto.push_str(&format!("  {linha}\n"));
            }
        }
        if self.pendencias.vazias() {
            texto.push_str("nada a apontar\n");
        }
        texto
    }

    /// O relatório inteiro, com desempenho, chamadas e o log do jogo.
    pub fn completo(&self) -> String {
        // O caminho fica fora do resumo de propósito: ele muda de máquina para máquina, e a
        // linha de base acusaria diferença só por isso.
        let mut texto = format!("arquivo: {}\n{}", self.arquivo.display(), self.resumo());
        if let Some(d) = &self.desempenho {
            texto.push_str(&format!(
                "desempenho:\n  \
                 abriu em {:.1} s, rodou {:.1} s reais para {} ms virtuais ({}% da velocidade do console)\n  \
                 {} volta(s), {} quadro(s) ({} fps virtuais)\n  \
                 {} instruções ({}/s), {} chamada(s) de API\n  \
                 {} KB de heap, {} objeto(s) vivo(s)\n",
                d.abertura.as_secs_f32(),
                d.laco.as_secs_f32(),
                d.virtual_ms,
                d.velocidade(),
                d.voltas,
                d.quadros,
                d.fps(),
                d.instrucoes,
                d.ips(),
                d.chamadas,
                d.heap / 1024,
                d.objetos,
            ));
        }
        if let Some(estado) = &self.estado_da_falha {
            texto.push_str(&format!("no instante da falha:\n{estado}"));
        }
        if !self.chamadas_maiores.is_empty() {
            texto.push_str("chamadas que mais pesaram:\n");
            for (nome, vezes) in &self.chamadas_maiores {
                texto.push_str(&format!("  {vezes:>8}x {nome}\n"));
            }
        }
        if !self.log_do_jogo.is_empty() {
            texto.push_str("log do jogo:\n");
            for linha in &self.log_do_jogo {
                texto.push_str(&format!("  {linha}\n"));
            }
        }
        texto
    }

    /// Uma linha por jogo, para a tabela da varredura.
    pub fn linha(&self) -> String {
        let desempenho = match &self.desempenho {
            Some(d) => format!(
                "{} ms virtuais em {:.1} s, {} quadro(s), {}%",
                d.virtual_ms,
                d.laco.as_secs_f32(),
                d.quadros,
                d.velocidade()
            ),
            None => "não rodou".to_string(),
        };
        let pendencias: usize = self
            .pendencias
            .secoes()
            .iter()
            .map(|(_, linhas)| linhas.len())
            .sum();
        format!(
            "{:<44} {:<26} {desempenho}, {pendencias} pendência(s)",
            self.titulo,
            self.categoria.rotulo()
        )
    }

    /// Quando nem houve sessão, o relatório é só o motivo da recusa.
    fn recusado(arquivo: &Path, erro: &StartError) -> Self {
        // A recusa é classificada pelo que ela diz do jogo, e não pela etapa em que ocorreu:
        // "não temos a classe que ele pede" e "não é um módulo" são problemas diferentes, e o
        // levantamento os separa.
        let categoria = match erro {
            StartError::NoApplet | StartError::Refused(_) => Categoria::SemApplet,
            StartError::Stopped(_) => Categoria::QuebrouAntesDoApplet,
            StartError::Unreadable(_) | StartError::NotAModule(_) | StartError::NotLoadable(_) => {
                Categoria::NaoCarrega
            }
        };
        Self {
            arquivo: arquivo.to_path_buf(),
            titulo: crate::library::title_for(arquivo),
            categoria,
            motivo: Some(erro.to_string()),
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: None,
            chamadas_maiores: Vec::new(),
            log_do_jogo: Vec::new(),
        }
    }
}

/// Os registradores e a pilha de quem quebrou, quando a parada foi acesso inválido ou exceção.
fn estado_da_falha(session: &Session) -> Option<String> {
    if !matches!(
        session.stopped(),
        Some(Outcome::Fault { .. } | Outcome::Exception { .. })
    ) {
        return None;
    }
    let machine = session.machine();
    let registradores: Vec<String> = machine
        .fault_regs()
        .iter()
        .enumerate()
        .map(|(i, valor)| format!("r{i}={valor:#x}"))
        .collect();
    let mut texto = format!("  {}\n", registradores.join(" "));
    let pilha = machine.fault_stack();
    if !pilha.is_empty() {
        let itens: Vec<String> = pilha.iter().map(|valor| format!("{valor:#x}")).collect();
        texto.push_str(&format!("  pilha: {}\n", itens.join(" ")));
    }
    Some(texto)
}

/// Carrega a ROM, entrega o `EVT_APP_START` e roda `ms_virtuais` de tempo de jogo.
///
/// O laço anda **uma volta por chamada** de [`Session::step`], com orçamento de tempo real
/// zerado: é o que permite dizer em que volta o jogo parou, como o relatório do `run` diz. O
/// freio de velocidade fica desligado, porque aqui ninguém está assistindo — o que se quer é o
/// tempo virtual cumprido o mais rápido que a máquina der.
pub fn examina(arquivo: &Path, ms_virtuais: u32, teto: Duration) -> Relatorio {
    let comeco = Instant::now();
    let mut session = match Session::start_with(arquivo, crate::PORTAS_PADRAO, None) {
        Ok(session) => session,
        Err(erro) => return Relatorio::recusado(arquivo, &erro),
    };
    let abertura = comeco.elapsed();
    let base_ms = session.clock_ms();
    let mut medida = Desempenho {
        abertura,
        ..Default::default()
    };

    // A partida vem separada da primeira volta: é nela que o applet recebe o `EVT_APP_START`.
    // Distinguir as duas é o que separa "quebrou antes do primeiro quadro" de "quebrou no
    // laço", e essas duas linhas do levantamento apontam para trechos de código diferentes —
    // sem a separação, um jogo que morre na primeira volta do laço aparecia como morto na
    // partida.
    let laco = Instant::now();
    let mut categoria = match session.partida() {
        Some(_) => Categoria::QuebrouNaPartida,
        None => Categoria::Roda,
    };

    if categoria == Categoria::Roda {
        loop {
            if session.clock_ms().saturating_sub(base_ms) >= ms_virtuais {
                break;
            }
            if laco.elapsed() >= teto {
                categoria = Categoria::LentoDemais;
                break;
            }
            medida.voltas += 1;
            match session.step(Duration::ZERO, false) {
                Step::Presented => medida.quadros += 1,
                Step::Stopped => {
                    categoria = Categoria::QuebrouNoLaco;
                    break;
                }
                Step::Running | Step::Ahead => {}
            }
        }
    }

    // Fim por `Returned` não é quebra: é o jogo tendo terminado o que tinha para fazer, e é o
    // desfecho de um `helloworld` que desenha e sai.
    if matches!(session.stopped(), Some(Outcome::Returned { .. })) {
        categoria = Categoria::Terminou;
    }

    medida.laco = laco.elapsed();
    medida.virtual_ms = session.clock_ms().saturating_sub(base_ms);
    medida.instrucoes = session.machine().instructions();
    let mut chamadas = session.machine().call_log();
    medida.chamadas = chamadas.iter().map(|(_, vezes)| vezes).sum();
    let (heap, objetos) = session.memory();
    medida.heap = heap;
    medida.objetos = objetos;
    chamadas.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    chamadas.truncate(CHAMADAS_MOSTRADAS);

    let mut log: Vec<String> = session
        .machine()
        .debug_output()
        .iter()
        .map(|(linha, vezes)| match vezes {
            1 => linha.clone(),
            n => format!("{linha}   ({n}x)"),
        })
        .collect();
    log.extend(
        session
            .machine()
            .cpu()
            .semihosting()
            .lines()
            .map(str::to_string),
    );
    let sobra = log.len().saturating_sub(LOG_MOSTRADO);
    log.drain(..sobra);

    Relatorio {
        arquivo: arquivo.to_path_buf(),
        // O título sai do arquivo que se pediu, e **não** do `Session::title`: para um `.zip` a
        // sessão nomeia o jogo pela pasta do cache, cujo nome carrega o tamanho e a data do
        // arquivo. Isso muda de máquina para máquina, e a linha de base — que é gravada com
        // esse nome e traz o título dentro — não pode depender de metadado do host.
        titulo: crate::library::title_for(arquivo),
        categoria,
        motivo: session.stopped_reason(),
        desempenho: Some(medida),
        pendencias: Pendencias::de(&session),
        estado_da_falha: estado_da_falha(&session),
        chamadas_maiores: chamadas,
        log_do_jogo: log,
    }
}

/// Só carrega a ROM e cria o applet, sem rodar o laço.
///
/// É a pergunta mais barata que se pode fazer de um jogo — "abre, ou estoura na cara?" —, e a
/// que dá para fazer de sessenta e cinco de uma vez sem esperar minutos. O que ela mede é o
/// caminho até o applet existir: carga do `.mod`, `AEEMod_Load`, `.mif` e `CreateInstance`.
pub fn abre(arquivo: &Path) -> Result<Duration, String> {
    let comeco = Instant::now();
    match Session::start_with(arquivo, crate::PORTAS_PADRAO, None) {
        Ok(_) => Ok(comeco.elapsed()),
        Err(erro) => Err(erro.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As ROMs que o ambiente pediu. Vazio quando ninguém pediu nenhuma.
    ///
    /// Aceita arquivo, lista separada por vírgula e diretório. O diretório é a varredura
    /// inteira, e ela **só acontece quando alguém a escreve** — nunca por padrão.
    fn roms_pedidas() -> Vec<PathBuf> {
        let Ok(valor) = std::env::var("ZEEBX_ROM") else {
            return Vec::new();
        };
        let mut caminhos = Vec::new();
        for parte in valor.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let caminho = PathBuf::from(parte);
            if !caminho.is_dir() {
                caminhos.push(caminho);
                continue;
            }
            let Ok(entradas) = std::fs::read_dir(&caminho) else {
                continue;
            };
            let mut jogos: Vec<PathBuf> = entradas
                .flatten()
                .map(|entrada| entrada.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("zip" | "mod" | "ZIP" | "MOD")
                    )
                })
                .collect();
            jogos.sort();
            caminhos.extend(jogos);
        }
        caminhos
    }

    fn numero(nome: &str, padrao: u64) -> u64 {
        std::env::var(nome)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(padrao)
    }

    fn diretorio(nome: &str) -> Option<PathBuf> {
        let caminho = PathBuf::from(std::env::var(nome).ok()?);
        std::fs::create_dir_all(&caminho).ok()?;
        Some(caminho)
    }

    /// Nome de arquivo para o título de um jogo, sem depender do sistema de arquivos do host.
    fn arquivo_do_titulo(titulo: &str) -> String {
        let limpo: String = titulo
            .chars()
            .map(|c| match c.is_ascii_alphanumeric() {
                true => c.to_ascii_lowercase(),
                false => '-',
            })
            .collect();
        format!("{}.txt", limpo.trim_matches('-'))
    }

    /// Roda `corpo` para cada ROM pedida e só falha no fim, com todas as falhas juntas.
    ///
    /// Falhar na primeira esconde as outras, e numa varredura o que se quer ver é o placar
    /// inteiro — um jogo que quebrou não diz nada sobre os outros sessenta e quatro.
    fn com_cada_rom(nome: &str, mut corpo: impl FnMut(&Path) -> Result<String, String>) {
        let roms = roms_pedidas();
        if roms.is_empty() {
            eprintln!(
                "{nome}: nenhuma ROM para examinar. Aponte ZEEBX_ROM para um arquivo, uma lista \
                 separada por vírgula ou um diretório, e rode de novo com --release."
            );
            return;
        }
        let mut falhas = Vec::new();
        for rom in &roms {
            match corpo(rom) {
                Ok(linha) => println!("ok    {linha}"),
                Err(erro) => {
                    println!("FALHA {erro}");
                    falhas.push(erro);
                }
            }
        }
        assert!(
            falhas.is_empty(),
            "{} de {} ROM(s) com problema:\n{}",
            falhas.len(),
            roms.len(),
            falhas.join("\n")
        );
    }

    /// A pergunta genérica: a ROM abre, ou estoura de cara — e com que erro.
    ///
    /// Vale por si: os cinco jogos que "não criam o applet" no levantamento falham aqui, sem
    /// gastar os seis segundos virtuais que eles nunca vão chegar a rodar.
    #[test]
    fn a_rom_indicada_abre() {
        com_cada_rom("a_rom_indicada_abre", |rom| {
            let titulo = crate::library::title_for(rom);
            match abre(rom) {
                Ok(quanto) => Ok(format!("{titulo}: abriu em {:.1} s", quanto.as_secs_f32())),
                Err(erro) => Err(format!("{titulo}: não abriu — {erro}")),
            }
        });
    }

    /// A ROM avança, e o que aconteceu vira relatório.
    ///
    /// Falha quando o jogo quebra, e quando o resumo difere da linha de base — que é o ponto
    /// todo: uma API que deixou de ser atendida, uma classe que passou a ser pedida ou um
    /// arquivo que sumiu aparecem aqui, no commit que os causou.
    #[test]
    fn a_rom_indicada_avanca() {
        let ms = numero("ZEEBX_ROM_MS", u64::from(MS_PADRAO)) as u32;
        let teto = Duration::from_secs(numero("ZEEBX_ROM_TETO", TETO_PADRAO));
        let saida = diretorio("ZEEBX_ROM_SAIDA");
        let base = diretorio("ZEEBX_ROM_BASE");
        com_cada_rom("a_rom_indicada_avanca", |rom| {
            let relatorio = examina(rom, ms, teto);
            println!("\n===== {} =====\n{}", rom.display(), relatorio.completo());
            let nome = arquivo_do_titulo(&relatorio.titulo);
            if let Some(dir) = &saida {
                let _ = std::fs::write(dir.join(&nome), relatorio.completo());
            }
            let mut queixas = Vec::new();
            if !relatorio.categoria.passa() {
                queixas.push(match &relatorio.motivo {
                    Some(motivo) => format!("{} — {motivo}", relatorio.categoria.rotulo()),
                    None => relatorio.categoria.rotulo().to_string(),
                });
            }
            if let Some(dir) = &base
                && let Err(diferenca) = confere_base(&dir.join(&nome), &relatorio)
            {
                queixas.push(diferenca);
            }
            match queixas.is_empty() {
                true => Ok(relatorio.linha()),
                false => Err(format!("{}\n  {}", relatorio.linha(), queixas.join("\n  "))),
            }
        });
    }

    /// Compara o resumo com a linha de base, gravando-a quando ela ainda não existe.
    ///
    /// Gravar o que falta, em vez de exigir que alguém escreva à mão, é o que torna possível
    /// adotar a linha de base de um jogo novo numa execução — e a diferença fica visível no
    /// `git diff`, que é onde ela deve ser revisada, jogo por jogo.
    fn confere_base(caminho: &Path, relatorio: &Relatorio) -> Result<(), String> {
        let atual = relatorio.resumo();
        let Ok(esperado) = std::fs::read_to_string(caminho) else {
            let _ = std::fs::write(caminho, &atual);
            println!("      linha de base gravada em {}", caminho.display());
            return Ok(());
        };
        if esperado == atual {
            return Ok(());
        }
        let linhas = |texto: &str| -> Vec<String> { texto.lines().map(str::to_string).collect() };
        let (antes, agora) = (linhas(&esperado), linhas(&atual));
        let mut diferenca = vec![format!("mudou desde {}:", caminho.display())];
        diferenca.extend(
            antes
                .iter()
                .filter(|l| !agora.contains(l))
                .map(|l| format!("  - {l}")),
        );
        diferenca.extend(
            agora
                .iter()
                .filter(|l| !antes.contains(l))
                .map(|l| format!("  + {l}")),
        );
        Err(diferenca.join("\n"))
    }

    // Daqui para baixo, testes que não precisam de ROM nenhuma: é a lógica da varredura sendo
    // verificada, e ela é justamente a parte que não dá para conferir olhando a saída de um
    // jogo. Sem eles, um erro de classificação passaria como resultado do jogo.

    #[test]
    fn um_arquivo_que_nao_e_modulo_vira_relatorio_com_motivo() {
        let caminho = std::env::temp_dir().join("zeebx-varredura-lixo.mod");
        std::fs::write(&caminho, b"isto nao e um modulo").unwrap();
        let relatorio = examina(&caminho, 10, Duration::from_secs(1));
        // O cabeçalho do `.mod` é frouxo: vinte bytes de texto passam pelo carregador e o que
        // recusa é o próprio ARM, ao executá-los. Por isso a categoria é "quebrou antes de
        // criar o applet" e não "não carrega" — e é essa a diferença que o relatório precisa
        // dizer, porque as duas apontam para lugares diferentes.
        assert_eq!(relatorio.categoria, Categoria::QuebrouAntesDoApplet);
        assert!(!relatorio.categoria.passa());
        // O motivo é o que se lê para saber o que fazer, então não pode vir vazio nem ficar de
        // fora do resumo — que é o texto que a linha de base guarda.
        assert!(relatorio.motivo.as_deref().is_some_and(|m| !m.is_empty()));
        assert!(relatorio.resumo().contains("quebrou antes de criar o applet"));
        assert!(relatorio.desempenho.is_none());
        let _ = std::fs::remove_file(&caminho);
    }

    #[test]
    fn so_roda_e_terminou_contam_como_teste_passando() {
        for categoria in [Categoria::Roda, Categoria::Terminou] {
            assert!(categoria.passa(), "{categoria:?} devia passar");
        }
        for categoria in [
            Categoria::NaoCarrega,
            Categoria::QuebrouAntesDoApplet,
            Categoria::SemApplet,
            Categoria::QuebrouNaPartida,
            Categoria::QuebrouNoLaco,
            Categoria::LentoDemais,
        ] {
            assert!(!categoria.passa(), "{categoria:?} não devia passar");
        }
    }

    /// A recusa é classificada pelo que ela diz do jogo, não pela etapa em que aconteceu.
    #[test]
    fn a_recusa_separa_quem_nao_carrega_de_quem_nao_cria_o_applet() {
        let arquivo = Path::new("/tmp/exemplo.mod");
        let caso = |erro: StartError| Relatorio::recusado(arquivo, &erro).categoria;
        assert_eq!(caso(StartError::NoApplet), Categoria::SemApplet);
        assert_eq!(caso(StartError::Refused(20)), Categoria::SemApplet);
        assert_eq!(
            caso(StartError::Stopped(Outcome::Budget)),
            Categoria::QuebrouAntesDoApplet
        );
        assert_eq!(
            caso(StartError::NotAModule("x".into())),
            Categoria::NaoCarrega
        );
        assert_eq!(
            caso(StartError::NotLoadable("x".into())),
            Categoria::NaoCarrega
        );
    }

    /// **A linha de base não pode enxergar desempenho.**
    ///
    /// Se enxergasse, trocar de máquina — ou rodar em depuração em vez de `--release` — acusaria
    /// regressão em todos os jogos de uma vez, e um teste que acusa sempre não é lido nunca.
    #[test]
    fn o_resumo_nao_muda_com_o_desempenho() {
        let modelo = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            categoria: Categoria::Roda,
            motivo: None,
            desempenho: Some(Desempenho {
                virtual_ms: 6_000,
                quadros: 300,
                instrucoes: 700_000_000,
                laco: Duration::from_secs(4),
                ..Default::default()
            }),
            pendencias: Pendencias {
                arquivos: vec!["config.cfg".to_string()],
                ..Default::default()
            },
            estado_da_falha: None,
            chamadas_maiores: vec![("IDisplay::Update".to_string(), 300)],
            log_do_jogo: vec!["carregando".to_string()],
        };
        let devagar = Relatorio {
            desempenho: Some(Desempenho {
                virtual_ms: 6_000,
                quadros: 41,
                instrucoes: 700_000_000,
                laco: Duration::from_secs(70),
                ..Default::default()
            }),
            chamadas_maiores: Vec::new(),
            log_do_jogo: Vec::new(),
            ..modelo.clone()
        };
        assert_eq!(modelo.resumo(), devagar.resumo());
        assert!(modelo.resumo().contains("config.cfg"));
        // O completo, sim, mostra a diferença — é o relatório que uma pessoa lê.
        assert_ne!(modelo.completo(), devagar.completo());
    }

    /// Uma pendência que aparece muda o resumo, e é isso que a linha de base cobra.
    #[test]
    fn uma_pendencia_nova_muda_o_resumo() {
        let limpo = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            categoria: Categoria::Roda,
            motivo: None,
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: None,
            chamadas_maiores: Vec::new(),
            log_do_jogo: Vec::new(),
        };
        assert!(limpo.resumo().contains("nada a apontar"));
        let com_api = Relatorio {
            pendencias: Pendencias {
                apis: vec!["IShell::Método (de 0x00012345)".to_string()],
                ..Default::default()
            },
            ..limpo.clone()
        };
        assert_ne!(limpo.resumo(), com_api.resumo());
        assert!(com_api.resumo().contains("APIs que faltaram"));
        assert!(!com_api.resumo().contains("nada a apontar"));
    }

    /// A linha de base é gravada na primeira vez e cobrada na segunda.
    #[test]
    fn a_linha_de_base_e_gravada_e_depois_cobrada() {
        let caminho = std::env::temp_dir().join("zeebx-varredura-base.txt");
        let _ = std::fs::remove_file(&caminho);
        let modelo = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            categoria: Categoria::Roda,
            motivo: None,
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: None,
            chamadas_maiores: Vec::new(),
            log_do_jogo: Vec::new(),
        };
        assert!(confere_base(&caminho, &modelo).is_ok(), "grava o que falta");
        assert!(confere_base(&caminho, &modelo).is_ok(), "e depois confere");
        let mudou = Relatorio {
            pendencias: Pendencias {
                classes: vec!["0x01003109".to_string()],
                ..Default::default()
            },
            ..modelo
        };
        let Err(diferenca) = confere_base(&caminho, &mudou) else {
            panic!("uma classe nova tinha de acusar diferença");
        };
        // A diferença precisa nomear o que apareceu: "mudou" sem dizer o quê não ajuda ninguém.
        assert!(diferenca.contains("+   0x01003109"), "{diferenca}");
        let _ = std::fs::remove_file(&caminho);
    }

    /// O estado da falha aparece no relatório completo e **não** no resumo.
    ///
    /// Registrador é material de investigação: dois computadores podem chegar ao mesmo defeito
    /// com pilha diferente, e cobrar isso na linha de base seria acusar regressão onde não há.
    #[test]
    fn o_estado_da_falha_fica_fora_do_resumo() {
        let relatorio = Relatorio {
            arquivo: PathBuf::from("/tmp/x.mod"),
            titulo: "Exemplo".to_string(),
            categoria: Categoria::QuebrouNoLaco,
            motivo: Some("acesso inválido a 0x00000024, em 0x00032b78".to_string()),
            desempenho: None,
            pendencias: Pendencias::default(),
            estado_da_falha: Some("  r0=0x0 r1=0x24\n  pilha: 0x1 0x2\n".to_string()),
            chamadas_maiores: Vec::new(),
            log_do_jogo: Vec::new(),
        };
        assert!(relatorio.completo().contains("no instante da falha"));
        assert!(relatorio.completo().contains("r1=0x24"));
        assert!(!relatorio.resumo().contains("r1=0x24"));
        // O motivo, sim, está nos dois: é ele que diz o que quebrou.
        assert!(relatorio.resumo().contains("0x00032b78"));
    }

    #[test]
    fn o_titulo_vira_nome_de_arquivo_previsivel() {
        assert_eq!(arquivo_do_titulo("Quake 2"), "quake-2.txt");
        assert_eq!(
            arquivo_do_titulo("Alice no País das Maravilhas"),
            "alice-no-pa-s-das-maravilhas.txt"
        );
    }
}
