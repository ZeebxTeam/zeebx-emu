//! O registro de mensagens do núcleo, em cinco níveis.
//!
//! **Por que existe.** O núcleo não tinha canal nenhum: o diagnóstico saía por `eprintln!` na
//! interface, e o jogo tinha o seu próprio log à parte. Quando o mesmo problema aparecia no
//! core Libretro, no Android ou na varredura, não havia como dizer "quero ver o nível de aviso
//! deste subsistema" — cada frontend imprimia o que dava, no formato que dava.
//!
//! Aqui a mensagem nasce no núcleo, com **nível** e **alvo** (o subsistema), e o frontend
//! decide para onde ela vai: `eprintln!` no desktop, `retro_log` no Libretro, `logcat` no
//! Android, a linha de base da varredura nos testes.
//!
//! **O filtro custa uma leitura atômica.** Quem chama usa a macro [`registro!`], que testa o
//! nível antes de montar o texto: um jogo que emite mil mensagens de depuração por quadro não
//! paga por elas quando o nível é `Aviso` — que é o padrão.
//!
//! **O coletor é um anel, e o anel avisa quando transborda.** Guardar tudo o que um jogo
//! depurado diz encheria a memória; guardar sem avisar esconderia justamente o trecho que
//! faltou. A contagem de descartes aparece no relatório.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Quanto cabe no anel antes de o registro começar a descartar o mais antigo.
///
/// Trezentas linhas cobrem com folga o arranque de um applet e uma troca de cena. É o mesmo
/// espírito do teto do log do jogo ([`crate::machine`]), e o motivo é o mesmo: o relatório é
/// para ser lido, não armazenado.
/// Quantas linhas o anel guarda antes de sobrescrever as mais antigas.
///
/// **O número é do instrumento, não do jogo.** Com 300, uma corrida de 40 s em que o jogo fala
/// muito no boot perde justamente as linhas do áudio antes de qualquer um as ler: o `run` drena o
/// anel no fim, e o que ele imprime é só o que sobrou. Foi assim que uma medição minha relatou
/// "zero sons decodificados" com 123 sons pedidos. Quatro mil linhas custam menos de um megabyte e
/// cobrem uma corrida inteira; os descartes continuam contados, e o relatório os mostra.
const CAPACIDADE: usize = 4096;

/// Os cinco níveis, do mais falador para o mais grave.
///
/// São os cinco que o `RETRO_LOG_LEVEL` do Libretro e o `log` do Rust esperam, e por isso a
/// ordem aqui é a mesma de lá: comparar níveis é comparar o número.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Nivel {
    /// O caminho de cada decisão. É o "verbose" e o "debug" do costume, juntos.
    Depuracao = 0,
    /// O que aconteceu e vale saber: abriu o jogo, escolheu o rasterizador, podou o cache.
    Informacao = 1,
    /// O que saiu do previsto mas não impede nada.
    Aviso = 2,
    /// O que falhou numa operação e foi tratado.
    Erro = 3,
    /// O que impede continuar.
    Fatal = 4,
}

impl Nivel {
    /// Todos, na ordem em que se apresentam numa lista de opções.
    pub const TODOS: [Nivel; 5] = [
        Nivel::Depuracao,
        Nivel::Informacao,
        Nivel::Aviso,
        Nivel::Erro,
        Nivel::Fatal,
    ];

    /// O nome canônico em português — o que fica guardado no ajuste da interface.
    ///
    /// **Serve para haver um só.** [`Nivel::de_texto`] aceita dois vocabulários, o do core
    /// (português) e o do `log` do Rust (inglês), e guardar o texto que veio faria o mesmo nível
    /// tomar duas formas no `settings.json` e no `config.ini`. Quem lê aceita as duas, mas dois
    /// ajustes iguais deixavam de ser iguais — e foi assim que o teste do `config.ini` de fábrica
    /// pegou o `log = warn` do arquivo contra o padrão `aviso` do emulador.
    ///
    /// Os nomes são os mesmos que o `zeebx_log` do core Libretro oferece, de propósito: um nível,
    /// um token, nos três lugares que o escrevem.
    pub fn nome(self) -> &'static str {
        match self {
            Nivel::Depuracao => "depuracao",
            Nivel::Informacao => "informacao",
            Nivel::Aviso => "aviso",
            Nivel::Erro => "erro",
            Nivel::Fatal => "fatal",
        }
    }

    /// O nome curto, em maiúsculas, como sai no log do frontend.
    pub fn etiqueta(self) -> &'static str {
        match self {
            Nivel::Depuracao => "DEBUG",
            Nivel::Informacao => "INFO",
            Nivel::Aviso => "WARN",
            Nivel::Erro => "ERROR",
            Nivel::Fatal => "FATAL",
        }
    }

    /// O nível a partir do texto de uma opção, em português ou no nome do `log` do Rust.
    ///
    /// Aceita os dois vocabulários de propósito: o usuário do core Libretro escreve
    /// `depuracao`, e quem lê o `ZEEBX_LOG` de um script costuma escrever `debug`.
    pub fn de_texto(texto: &str) -> Option<Self> {
        match texto.trim().to_ascii_lowercase().as_str() {
            "depuracao" | "depuração" | "debug" | "verbose" => Some(Nivel::Depuracao),
            "informacao" | "informação" | "info" => Some(Nivel::Informacao),
            "aviso" | "warn" | "warning" => Some(Nivel::Aviso),
            "erro" | "error" => Some(Nivel::Erro),
            "fatal" | "critical" | "critico" | "crítico" => Some(Nivel::Fatal),
            _ => None,
        }
    }

    /// Se este nível passa pelo filtro de `ativo`.
    ///
    /// Função pura de propósito: é a regra que mais erra quando se escreve à mão, e assim ela
    /// tem teste sem tocar no global.
    pub fn passa(self, ativo: Nivel) -> bool {
        self >= ativo
    }
}

/// O que a opção pediu: um teto de gravidade, ou nada.
///
/// **Existe porque "desligado" e "não entendi" não são a mesma coisa.** Devolver `None` para os
/// dois faria um `ZEEBX_LOG=banana` desligar o registro em silêncio, que é o pior desfecho
/// possível para uma opção de depuração: quem escreveu errado precisa saber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ajuste {
    /// Não registrar nada.
    Desligado,
    /// Registrar deste nível para cima.
    Ate(Nivel),
}

impl Ajuste {
    /// O ajuste a partir do texto de uma opção. `None` é "não é token nenhum".
    pub fn de_texto(texto: &str) -> Option<Self> {
        match texto.trim().to_ascii_lowercase().as_str() {
            "desligado" | "off" | "nenhum" => Some(Ajuste::Desligado),
            outro => Nivel::de_texto(outro).map(Ajuste::Ate),
        }
    }

    /// Aplica o ajuste ao registro global.
    pub fn aplica(self) {
        match self {
            Ajuste::Desligado => desliga(),
            Ajuste::Ate(nivel) => define_nivel(nivel),
        }
    }
}

/// Até que nível o registro está ligado. O padrão é [`Nivel::Aviso`].
///
/// `Aviso` e não `Depuracao` porque instrumentar não pode custar caro a quem não pediu: quem
/// não mexe em nada continua com o silêncio de antes, e só vê o que já via.
static ATIVO: AtomicU8 = AtomicU8::new(Nivel::Aviso as u8);

/// Uma linha guardada, à espera de quem a vá mostrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linha {
    /// A gravidade.
    pub nivel: Nivel,
    /// O subsistema que escreveu — `"cpu"`, `"gl"`, `"midi"`, `"loader"`.
    pub alvo: String,
    /// O texto, já montado.
    pub texto: String,
}

/// O anel. Um só para todo o núcleo, porque é um só o log que interessa ler.
static LINHAS: Mutex<VecDeque<Linha>> = Mutex::new(VecDeque::new());

/// Quantas linhas foram descartadas por o anel estar cheio. Nunca zera sozinho: quem lê é que
/// zera, com [`zera_descartes`], para não se perder a conta entre duas leituras.
static DESCARTES: AtomicU64 = AtomicU64::new(0);

/// Se o registro está desligado por completo.
static DESLIGADO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Passa a registrar a partir de `nivel`, inclusive.
pub fn define_nivel(nivel: Nivel) {
    DESLIGADO.store(false, Ordering::Relaxed);
    ATIVO.store(nivel as u8, Ordering::Relaxed);
}

/// Desliga o registro por completo, seja qual for o nível.
pub fn desliga() {
    DESLIGADO.store(true, Ordering::Relaxed);
}

/// O nível ativo agora.
pub fn nivel() -> Nivel {
    match ATIVO.load(Ordering::Relaxed) {
        0 => Nivel::Depuracao,
        1 => Nivel::Informacao,
        2 => Nivel::Aviso,
        3 => Nivel::Erro,
        _ => Nivel::Fatal,
    }
}

/// Se uma mensagem deste nível seria registrada.
///
/// É o teste barato que a macro [`registro!`] faz antes de montar o texto — e é o único custo
/// de uma mensagem filtrada.
pub fn ligado(nivel_da_mensagem: Nivel) -> bool {
    !DESLIGADO.load(Ordering::Relaxed) && nivel_da_mensagem.passa(nivel())
}

/// Põe uma linha no anel.
///
/// Não escreve em lugar nenhum: quem mostra é o frontend, chamando [`drena`] num ponto seguro
/// — no Libretro, dentro do `retro_run`, porque o log do frontend não pode ser chamado de
/// qualquer lugar.
pub fn escreve(nivel: Nivel, alvo: &str, texto: &str) {
    if !ligado(nivel) {
        return;
    }
    let linha = Linha {
        nivel,
        alvo: alvo.to_string(),
        texto: texto.to_string(),
    };
    // Um cadeado envenenado aqui não é motivo para derrubar o emulador: o log é instrumento, e
    // instrumento que falha cala a boca em vez de matar o jogo.
    let Ok(mut linhas) = LINHAS.lock() else {
        return;
    };
    if poe_no_anel(&mut linhas, linha) {
        DESCARTES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Põe a linha no anel, tirando a mais antiga quando ele já está cheio.
///
/// Devolve `true` quando **descartou** alguma coisa. É função de fora do global de propósito:
/// a regra do teto é a que mais precisa de prova, e prová-la contra o anel do processo faz a
/// prova depender de quem mais estiver escrevendo nele.
fn poe_no_anel(anel: &mut VecDeque<Linha>, linha: Linha) -> bool {
    let descartou = anel.len() >= CAPACIDADE;
    if descartou {
        anel.pop_front();
    }
    anel.push_back(linha);
    descartou
}

/// Tira tudo o que está no anel, na ordem em que foi escrito.
pub fn drena() -> Vec<Linha> {
    match LINHAS.lock() {
        Ok(mut linhas) => linhas.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

/// Quantas linhas foram descartadas desde o último [`zera_descartes`].
pub fn descartes() -> u64 {
    DESCARTES.load(Ordering::Relaxed)
}

/// Zera a contagem de descartes.
pub fn zera_descartes() {
    DESCARTES.store(0, Ordering::Relaxed);
}

/// Esvazia o anel e a contagem de descartes.
pub fn limpa() {
    if let Ok(mut linhas) = LINHAS.lock() {
        linhas.clear();
    }
    zera_descartes();
}

/// Lê o nível de `ZEEBX_LOG` e o aplica, quando a variável existe.
///
/// Vale para todo frontend de uma vez — headless, varredura, desktop — sem depender de opção
/// de interface, que é o que serve a quem está depurando na linha de comando.
pub fn le_do_ambiente() {
    let Ok(valor) = std::env::var("ZEEBX_LOG") else {
        return;
    };
    match Ajuste::de_texto(&valor) {
        Some(ajuste) => ajuste.aplica(),
        None => escreve(
            Nivel::Aviso,
            "registro",
            &format!("ZEEBX_LOG=`{valor}` não é um nível; seguindo no padrão"),
        ),
    }
}

/// Mostra no `stderr` o que está no anel, no formato `NÍVEL alvo: texto`.
///
/// É o canal dos frontends que não têm um log próprio para receber as linhas — o desktop e o
/// headless —, e o mesmo que a varredura usa para o relatório. O Libretro **não** usa este
/// caminho: lá o destino é o `retro_log` do frontend, que é quem decide o arquivo e a
/// verbosidade, e é por isso que o despejo mora no core e não aqui.
pub fn despeja_no_stderr() {
    for linha in drena() {
        eprintln!("{} {}: {}", linha.nivel.etiqueta(), linha.alvo, linha.texto);
    }
    let descartes = descartes();
    if descartes > 0 {
        zera_descartes();
        eprintln!("WARN registro: {descartes} linha(s) descartada(s) pelo teto do anel");
    }
}

/// Registra uma mensagem, se o nível passar pelo filtro.
///
/// O `if` fica do lado de fora do `format!` de propósito: uma mensagem de depuração filtrada
/// não deve custar nem a montagem do texto.
#[macro_export]
macro_rules! registro {
    ($nivel:expr, $alvo:expr, $($arg:tt)*) => {
        if $crate::registro::ligado($nivel) {
            $crate::registro::escreve($nivel, $alvo, &format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializa as provas que mexem no anel e no nível globais.
    ///
    /// As provas de um binário rodam em paralelo, e o registro é um só para todo o processo: sem
    /// isto, uma prova que drena rouba as linhas da outra, e a que mede transbordo nunca enche.
    /// Foi assim que a primeira versão desta prova passou sozinha e falhou na suíte.
    static CADEADO: Mutex<()> = Mutex::new(());

    /// O cadeado da suíte, tolerante a uma prova que entrou em pânico antes de soltá-lo.
    fn sozinho() -> std::sync::MutexGuard<'static, ()> {
        CADEADO.lock().unwrap_or_else(|envenenado| envenenado.into_inner())
    }

    /// A linha de prova, com um alvo que não colide com o de ninguém.
    fn linha(alvo: &str, texto: &str) -> Linha {
        Linha {
            nivel: Nivel::Informacao,
            alvo: alvo.to_string(),
            texto: texto.to_string(),
        }
    }

    #[test]
    fn o_texto_da_opcao_vira_o_nivel_certo() {
        assert_eq!(Nivel::de_texto("depuracao"), Some(Nivel::Depuracao));
        assert_eq!(Nivel::de_texto("DEBUG"), Some(Nivel::Depuracao));
        assert_eq!(Nivel::de_texto("verbose"), Some(Nivel::Depuracao));
        assert_eq!(Nivel::de_texto("info"), Some(Nivel::Informacao));
        assert_eq!(Nivel::de_texto(" warning "), Some(Nivel::Aviso));
        assert_eq!(Nivel::de_texto("erro"), Some(Nivel::Erro));
        assert_eq!(Nivel::de_texto("critical"), Some(Nivel::Fatal));
        assert_eq!(Nivel::de_texto("banana"), None);
        // Desligado não é nível: quem trata os dois iguais desliga o log por engano.
        assert_eq!(Nivel::de_texto("desligado"), None);
    }

    /// "Desligado" e "não entendi" têm de ser distinguíveis, senão um erro de escrita desliga o
    /// registro sem avisar.
    #[test]
    fn desligado_e_texto_invalido_nao_se_confundem() {
        assert_eq!(Ajuste::de_texto("desligado"), Some(Ajuste::Desligado));
        assert_eq!(Ajuste::de_texto("off"), Some(Ajuste::Desligado));
        assert_eq!(Ajuste::de_texto("banana"), None);
        assert_eq!(Ajuste::de_texto("aviso"), Some(Ajuste::Ate(Nivel::Aviso)));
    }

    #[test]
    fn o_ajuste_liga_e_desliga_o_registro() {
        let _sozinho = sozinho();
        Ajuste::Ate(Nivel::Erro).aplica();
        assert_eq!(nivel(), Nivel::Erro);
        assert!(ligado(Nivel::Erro));
        assert!(!ligado(Nivel::Aviso));

        Ajuste::Desligado.aplica();
        assert!(!ligado(Nivel::Fatal), "desligado não registra nem o fatal");

        define_nivel(Nivel::Aviso);
    }

    /// O filtro é a regra que mais erra à mão: o nível ativo e o nível da mensagem são a mesma
    /// escala, e passar significa "é igual ou mais grave".
    #[test]
    fn o_filtro_deixa_passar_do_nivel_ativo_para_cima() {
        let aviso = Nivel::Aviso;
        assert!(!Nivel::Depuracao.passa(aviso), "depuração é mais falante");
        assert!(!Nivel::Informacao.passa(aviso));
        assert!(Nivel::Aviso.passa(aviso), "o próprio nível passa");
        assert!(Nivel::Erro.passa(aviso));
        assert!(Nivel::Fatal.passa(aviso));

        let tudo = Nivel::Depuracao;
        assert!(Nivel::TODOS.iter().all(|n| n.passa(tudo)));
    }

    /// O `nome` é o token canônico, e os outros dois vocabulários continuam entrando.
    ///
    /// É o que faz `log = warn` no `config.ini` e `aviso` no `settings.json` serem o **mesmo**
    /// ajuste — antes, o arquivo de fábrica do frontend sem janela descrevia um padrão que não era
    /// o do emulador, e a bateria dele ficava vermelha por isso.
    #[test]
    fn o_nome_canonico_e_o_token_de_um_so_nivel() {
        assert_eq!(Nivel::Aviso.nome(), "aviso");
        assert_eq!(Nivel::Depuracao.nome(), "depuracao");
        for nivel in Nivel::TODOS {
            assert_eq!(
                Nivel::de_texto(nivel.nome()),
                Some(nivel),
                "{}",
                nivel.nome()
            );
            assert_eq!(
                Nivel::de_texto(nivel.etiqueta()),
                Some(nivel),
                "a etiqueta do log também é aceita"
            );
        }
        // Os dois vocabulários do lado de fora continuam valendo, e caem no mesmo nome.
        assert_eq!(Nivel::de_texto("warn"), Some(Nivel::Aviso));
        assert_eq!(Nivel::de_texto("warning"), Some(Nivel::Aviso));
        assert_eq!(Nivel::de_texto("DEBUG"), Some(Nivel::Depuracao));
        assert_eq!(Nivel::de_texto("banana"), None);
    }

    #[test]
    fn a_etiqueta_e_a_do_log_do_rust() {
        assert_eq!(Nivel::Depuracao.etiqueta(), "DEBUG");
        assert_eq!(Nivel::Informacao.etiqueta(), "INFO");
        assert_eq!(Nivel::Aviso.etiqueta(), "WARN");
        assert_eq!(Nivel::Erro.etiqueta(), "ERROR");
        assert_eq!(Nivel::Fatal.etiqueta(), "FATAL");
    }

    /// O anel guarda na ordem e o `drena` esvazia. Não usa `limpa` no começo para não apagar o
    /// que outra prova esteja olhando: procura as suas próprias linhas.
    #[test]
    fn o_anel_guarda_na_ordem_e_o_drena_esvazia() {
        let _sozinho = sozinho();
        limpa();
        define_nivel(Nivel::Depuracao);
        escreve(Nivel::Informacao, "teste-do-anel", "primeira");
        escreve(Nivel::Erro, "teste-do-anel", "segunda");

        let minhas: Vec<Linha> = drena()
            .into_iter()
            .filter(|l| l.alvo == "teste-do-anel")
            .collect();
        assert_eq!(minhas.len(), 2);
        assert_eq!(minhas[0].texto, "primeira");
        assert_eq!(minhas[1].texto, "segunda");
        assert_eq!(minhas[1].nivel, Nivel::Erro);

        // Depois de drenar, o que estava lá não volta.
        let outra_vez: Vec<Linha> = drena()
            .into_iter()
            .filter(|l| l.alvo == "teste-do-anel")
            .collect();
        assert!(outra_vez.is_empty());
    }

    /// **A regra do teto, sem o global.** O anel tem de descartar a mais antiga e dizer que
    /// descartou: transbordar em silêncio esconderia justamente o trecho que faltou.
    ///
    /// A regra é provada aqui, e não contra o anel do processo, porque o anel do processo é
    /// compartilhado: a primeira versão desta prova enchia 320 linhas e falhava na suíte, porque
    /// outra prova drenava no meio.
    #[test]
    fn o_anel_transborda_pela_ponta_e_conta_o_descarte() {
        let mut anel: VecDeque<Linha> = VecDeque::new();
        let mut descartes = 0;

        for i in 0..CAPACIDADE {
            assert!(!poe_no_anel(&mut anel, linha("teto", &format!("linha {i}"))));
        }
        assert_eq!(anel.len(), CAPACIDADE, "cabe exatamente o teto");

        // Uma a mais: sai a primeira, entra a nova.
        assert!(poe_no_anel(&mut anel, linha("teto", "a que chega")));
        descartes += 1;
        assert_eq!(anel.len(), CAPACIDADE, "o teto não cresce");
        assert_eq!(descartes, 1);
        assert_eq!(
            anel.front().map(|l| l.texto.as_str()),
            Some("linha 1"),
            "quem saiu foi a mais antiga"
        );
        assert_eq!(anel.back().map(|l| l.texto.as_str()), Some("a que chega"));

        // Vinte a mais: vinte descartes, e o tamanho continua no teto.
        for i in 0..20 {
            if poe_no_anel(&mut anel, linha("teto", &format!("extra {i}"))) {
                descartes += 1;
            }
        }
        assert_eq!(descartes, 21);
        assert_eq!(anel.len(), CAPACIDADE);
    }

    /// O contador de descartes do anel global aparece e some quando alguém o zera.
    #[test]
    fn o_contador_de_descartes_aparece_e_zera() {
        let _sozinho = sozinho();
        limpa();
        zera_descartes();
        // **O nível tem de ser fixado aqui.** Sem isto a prova dependia do que a anterior deixou:
        // escrita em `Informacao` com o nível em `Aviso` não entra no anel, e não há descarte
        // nenhum para contar. Passou até a ordem das provas mudar.
        define_nivel(Nivel::Depuracao);
        for i in 0..(CAPACIDADE + 5) {
            escreve(Nivel::Informacao, "teste-do-teto", &format!("linha {i}"));
        }
        assert!(descartes() >= 5, "os descartes têm de aparecer");
        zera_descartes();
        assert_eq!(descartes(), 0);
        limpa();
    }

    /// O despejo esvazia o anel, e chamá-lo com o anel vazio não faz nada.
    #[test]
    fn o_despejo_esvazia_o_anel() {
        let _sozinho = sozinho();
        define_nivel(Nivel::Depuracao);
        escreve(Nivel::Aviso, "teste-do-despejo", "para o stderr");
        despeja_no_stderr();
        assert!(
            !drena().iter().any(|l| l.alvo == "teste-do-despejo"),
            "depois de despejar, a linha não volta"
        );
        // Segunda chamada sem nada pendente: não pode entrar em pânico nem inventar linha.
        despeja_no_stderr();
    }

    /// A mensagem filtrada não entra no anel: é o que sustenta o argumento de custo.
    #[test]
    fn mensagem_abaixo_do_nivel_nao_entra_no_anel() {
        let _sozinho = sozinho();
        limpa();
        define_nivel(Nivel::Fatal);
        assert!(!ligado(Nivel::Informacao));
        escreve(Nivel::Informacao, "teste-filtrado", "não devia entrar");
        let achou = drena().iter().any(|l| l.alvo == "teste-filtrado");
        assert!(!achou, "o filtro tem de valer antes de guardar");

        define_nivel(Nivel::Depuracao);
        assert!(ligado(Nivel::Informacao));
    }
}
