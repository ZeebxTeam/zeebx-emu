//! Testes com servidor simulado: o JSON é o que o servidor do RA devolve, copiado dos testes do
//! próprio rcheevos (`test/client/test_rc_client.c`), com o console trocado para o Zeebo.

use super::*;
use std::collections::HashMap;

/// O que o servidor simulado viu e o que ele responde, por valor de `r=`.
#[derive(Default)]
struct Roteiro {
    respostas: HashMap<&'static str, String>,
    pedidos: Vec<Pedido>,
    /// Quando ligado, os pedidos são descartados sem resposta.
    mudo: bool,
    /// Quando ligado, as entregas ficam guardadas aqui sem resposta, como numa rede lenta.
    segura: bool,
    retidas: Vec<(String, Entrega)>,
    /// O user agent que o cliente passou ao servidor.
    user_agent: String,
    /// Os valores de `r=` que respondem como se não houvesse rede.
    sem_rede: Vec<&'static str>,
}

struct ServidorSimulado(Rc<RefCell<Roteiro>>);

impl Servidor for ServidorSimulado {
    fn define_user_agent(&mut self, user_agent: &str) {
        self.0.borrow_mut().user_agent = user_agent.to_string();
    }

    fn pede(&mut self, pedido: Pedido, entrega: Entrega) {
        let mut roteiro = self.0.borrow_mut();
        let r = parametro(&pedido, "r").unwrap_or_default();
        roteiro.pedidos.push(pedido);
        if roteiro.mudo {
            return;
        }
        if roteiro.segura {
            roteiro.retidas.push((r, entrega));
            return;
        }
        if roteiro.sem_rede.contains(&r.as_str()) {
            entrega.falha_de_rede("sem rede");
            return;
        }
        let corpo = roteiro
            .respostas
            .get(r.as_str())
            .cloned()
            .unwrap_or_else(|| {
                format!("{{\"Success\":false,\"Error\":\"sem resposta simulada para {r}\"}}")
            });
        entrega.responde(200, corpo.into_bytes());
    }
}

/// O valor de um parâmetro do pedido, no corpo do POST ou na URL.
fn parametro(pedido: &Pedido, nome: &str) -> Option<String> {
    let consulta = pedido
        .corpo
        .clone()
        .or_else(|| pedido.url.split_once('?').map(|(_, q)| q.to_string()))?;
    consulta.split('&').find_map(|par| {
        let (chave, valor) = par.split_once('=')?;
        (chave == nome).then(|| valor.to_string())
    })
}

/// Memória de mentira: `bytes` a partir de `base`.
struct MemoriaFalsa {
    base: u32,
    bytes: Vec<u8>,
}

impl Memoria for MemoriaFalsa {
    fn le(&self, endereco: u32, destino: &mut [u8]) -> bool {
        let Some(inicio) = endereco.checked_sub(self.base) else {
            return false;
        };
        let inicio = inicio as usize;
        match self.bytes.get(inicio..inicio + destino.len()) {
            Some(origem) => {
                destino.copy_from_slice(origem);
                true
            }
            None => false,
        }
    }
}

const LOGIN: &str = "{\"Success\":true,\"User\":\"User\",\"Token\":\"ApiToken\",\"Score\":12345,\
    \"SoftcoreScore\":123,\"Messages\":2,\"Permissions\":1,\"AccountType\":\"Registered\"}";

/// Duas conquistas: a 5501 quer o byte `0x1001` igual a 3 e o `0x1002` igual a 7; a 5502 lê
/// `0x9000`, fora da memória de mentira.
const CONJUNTO: &str = "{\"Success\":true,\
    \"GameId\":1234,\"Title\":\"Jogo de Teste\",\"ConsoleId\":70,\
    \"ImageIconUrl\":\"http://server/Images/112233.png\",\
    \"RichPresenceGameId\":1234,\"RichPresencePatch\":\"\",\"Sets\":[{\
      \"AchievementSetId\":1111,\"GameId\":1234,\"Title\":null,\"Type\":\"core\",\
      \"ImageIconUrl\":\"http://server/Images/112233.png\",\
      \"Achievements\":[\
       {\"ID\":5501,\"Title\":\"Primeira\",\"Description\":\"Desc1\",\"Flags\":3,\"Points\":5,\
        \"MemAddr\":\"0xH1001=3_0xH1002=7\",\"Author\":\"User1\",\"BadgeName\":\"00234\",\
        \"Created\":1367266583,\"Modified\":1376929305},\
       {\"ID\":5502,\"Title\":\"Fora do mapa\",\"Description\":\"Desc2\",\"Flags\":3,\"Points\":2,\
        \"MemAddr\":\"0xH9000=1\",\"Author\":\"User1\",\"BadgeName\":\"00235\",\
        \"Created\":1376970283,\"Modified\":1376970283}\
      ],\
      \"Leaderboards\":[]\
    }]}";

const SEM_DESBLOQUEIOS: &str = "{\"Success\":true,\"Unlocks\":[],\"HardcoreUnlocks\":[]}";

const DESCONHECIDO: &str =
    "{\"Success\":false,\"Error\":\"Unknown game\",\"Code\":\"not_found\",\"Status\":404}";

const DESBLOQUEIO: &str = "{\"Success\":true,\"Score\":5432,\"SoftcoreScore\":777,\
    \"AchievementID\":5501,\"AchievementsRemaining\":1}";

fn cliente(respostas: &[(&'static str, &str)]) -> (Conquistas, Rc<RefCell<Roteiro>>) {
    let roteiro = Rc::new(RefCell::new(Roteiro {
        respostas: respostas
            .iter()
            .map(|(r, corpo)| (*r, corpo.to_string()))
            .collect(),
        ..Default::default()
    }));
    let conquistas = Conquistas::novo(Box::new(ServidorSimulado(Rc::clone(&roteiro))));
    (conquistas, roteiro)
}

/// Um cliente já conectado, com o servidor pronto para carregar o jogo de teste. Para os testes
/// da sessão.
pub(crate) fn cliente_conectado_com_o_jogo_de_teste() -> Compartilhado {
    let (mut c, _) = cliente(&[
        ("login2", LOGIN),
        ("achievementsets", CONJUNTO),
        ("startsession", SEM_DESBLOQUEIOS),
        ("awardachievement", DESBLOQUEIO),
    ]);
    c.entra_com_senha("User", "Pa$$word");
    assert_eq!(c.eventos(), vec![Evento::Entrou(Ok(()))]);
    Rc::new(RefCell::new(c))
}

/// Um cliente conectado cujo jogo tem uma conquista só, a 7001, com a condição `condicao` na
/// sintaxe do RA. Para testar com um jogo de verdade.
pub(crate) fn cliente_conectado_com_a_condicao(condicao: &str) -> Compartilhado {
    let conjunto = CONJUNTO
        .replace("0xH1001=3_0xH1002=7", condicao)
        .replace("\"ID\":5501", "\"ID\":7001");
    let roteiro = Rc::new(RefCell::new(Roteiro::default()));
    {
        let mut r = roteiro.borrow_mut();
        r.respostas.insert("login2", LOGIN.into());
        r.respostas.insert("achievementsets", conjunto);
        r.respostas.insert("startsession", SEM_DESBLOQUEIOS.into());
        r.respostas.insert("awardachievement", DESBLOQUEIO.into());
    }
    let mut c = Conquistas::novo(Box::new(ServidorSimulado(roteiro)));
    c.entra_com_senha("User", "Pa$$word");
    assert_eq!(c.eventos(), vec![Evento::Entrou(Ok(()))]);
    Rc::new(RefCell::new(c))
}

/// Conectado, com o jogo de teste carregado e ativado, e a conquista 5501 trocada por
/// `condicao`. A memória fica zerada no quadro da ativação.
fn jogo_com_a_condicao(condicao: &str, memoria: &MemoriaFalsa) -> Conquistas {
    let conjunto = CONJUNTO.replace("0xH1001=3_0xH1002=7", condicao);
    let (mut c, _) = cliente(&[
        ("login2", LOGIN),
        ("achievementsets", conjunto.as_str()),
        ("startsession", SEM_DESBLOQUEIOS),
        ("awardachievement", DESBLOQUEIO),
    ]);
    c.entra_com_senha("User", "Pa$$word");
    c.carrega_jogo("0123456789abcdef0123456789abcdef");
    c.quadro(memoria);
    assert_eq!(
        c.eventos(),
        vec![Evento::Entrou(Ok(())), Evento::JogoCarregado(Ok(()))]
    );
    c
}

fn memoria() -> MemoriaFalsa {
    MemoriaFalsa {
        base: 0x1000,
        bytes: vec![0; 0x100],
    }
}

/// Entra, carrega o jogo de teste e roda o primeiro quadro, que é quando o jogo é ativado.
fn com_jogo_carregado(memoria: &MemoriaFalsa) -> (Conquistas, Rc<RefCell<Roteiro>>) {
    let (mut c, roteiro) = cliente(&[
        ("login2", LOGIN),
        ("achievementsets", CONJUNTO),
        ("startsession", SEM_DESBLOQUEIOS),
        ("awardachievement", DESBLOQUEIO),
    ]);
    c.entra_com_senha("User", "Pa$$word");
    c.carrega_jogo("0123456789abcdef0123456789abcdef");
    c.quadro(memoria);
    assert_eq!(
        c.eventos(),
        vec![Evento::Entrou(Ok(())), Evento::JogoCarregado(Ok(()))]
    );
    (c, roteiro)
}

#[test]
fn o_mapa_provisorio_e_a_identidade() {
    let mapa = Mapa::provisorio();
    assert_eq!(mapa.traduz(0x1000_0040, 4), Some((0x1000_0040, 4)));
    assert_eq!(mapa.traduz(u32::MAX, 4), Some((u32::MAX, 1)));
}

#[test]
fn o_mapa_corta_a_leitura_no_fim_da_faixa_e_recusa_o_buraco() {
    let mapa = Mapa::de(vec![
        Faixa {
            ra: 0x10_0000,
            guest: 0x1000_0000,
            tamanho: 0x100,
        },
        Faixa {
            ra: 0,
            guest: 0x0001_0000,
            tamanho: 0x1000,
        },
    ]);
    assert_eq!(mapa.traduz(0x0ffe, 4), Some((0x0001_0ffe, 2)));
    assert_eq!(mapa.traduz(0x10_0010, 4), Some((0x1000_0010, 4)));
    assert_eq!(mapa.traduz(0x2000, 1), None);
}

/// O formato que o RA cobra: nome e versão do emulador, o sistema, e a versão do rcheevos.
#[test]
fn o_user_agent_diz_o_zeebx_o_sistema_e_o_rcheevos() {
    let (c, _) = cliente(&[]);
    let ua = c.user_agent();
    let esperado = format!("Zeebx/{} (", env!("CARGO_PKG_VERSION"));
    assert!(ua.starts_with(&esperado), "{ua}");
    assert!(ua.contains(") rcheevos/"), "{ua}");
}

#[test]
fn o_cliente_nasce_em_casual() {
    let (c, _) = cliente(&[]);
    assert!(!c.hardcore());
}

#[test]
fn entrar_com_senha_guarda_o_token_e_nao_a_senha() {
    let (mut c, roteiro) = cliente(&[("login2", LOGIN)]);
    c.entra_com_senha("User", "Pa$$word");
    assert_eq!(c.eventos(), vec![Evento::Entrou(Ok(()))]);
    let usuario = c.usuario().expect("conectado");
    assert_eq!(usuario.nome, "User");
    assert_eq!(usuario.token, "ApiToken");
    assert_eq!(usuario.pontos, 12345);
    assert_eq!(usuario.pontos_casual, 123);

    let pedido = &roteiro.borrow().pedidos[0];
    assert_eq!(parametro(pedido, "u").as_deref(), Some("User"));
    assert_eq!(parametro(pedido, "p").as_deref(), Some("Pa%24%24word"));
}

#[test]
fn a_senha_errada_chega_como_erro_com_o_texto_do_servidor() {
    let (mut c, _) = cliente(&[(
        "login2",
        "{\"Success\":false,\"Error\":\"Invalid User/Password combination.\",\
         \"Code\":\"invalid_credentials\",\"Status\":401}",
    )]);
    c.entra_com_senha("User", "errada");
    let eventos = c.eventos();
    assert!(
        matches!(&eventos[..], [Evento::Entrou(Err(texto))] if texto.contains("Invalid User/Password")),
        "{eventos:?}"
    );
    assert!(c.usuario().is_none());
}

#[test]
fn entrar_com_token_manda_o_token() {
    let (mut c, roteiro) = cliente(&[("login2", LOGIN)]);
    c.entra_com_token("User", "ApiToken");
    assert_eq!(c.eventos(), vec![Evento::Entrou(Ok(()))]);
    let pedido = &roteiro.borrow().pedidos[0];
    assert_eq!(parametro(pedido, "t").as_deref(), Some("ApiToken"));
    assert_eq!(parametro(pedido, "p"), None);
}

/// Um servidor que perde o pedido não pode deixar o login pendurado para sempre.
#[test]
fn um_pedido_descartado_sem_resposta_vira_falha() {
    let (mut c, roteiro) = cliente(&[]);
    roteiro.borrow_mut().mudo = true;
    c.entra_com_senha("User", "Pa$$word");
    let eventos = c.eventos();
    assert!(
        matches!(&eventos[..], [Evento::Entrou(Err(_))]),
        "{eventos:?}"
    );
}

#[test]
fn o_jogo_carregado_diz_o_titulo_e_o_hash() {
    let memoria = memoria();
    let (c, roteiro) = com_jogo_carregado(&memoria);
    let jogo = c.jogo().expect("carregado");
    assert_eq!(jogo.id, 1234);
    assert_eq!(jogo.titulo, "Jogo de Teste");
    assert_eq!(jogo.hash, "0123456789abcdef0123456789abcdef");

    let roteiro = roteiro.borrow();
    let sessao = roteiro
        .pedidos
        .iter()
        .find(|p| parametro(p, "r").as_deref() == Some("startsession"))
        .expect("a sessão foi aberta");
    assert_eq!(parametro(sessao, "h").as_deref(), Some("0"), "casual");
}

/// A conquista só dispara depois de ter sido falsa num quadro (o rcheevos a "arma"), e o
/// desbloqueio vai ao servidor marcado como casual.
#[test]
fn a_conquista_dispara_pela_memoria_e_o_desbloqueio_e_casual() {
    let mut memoria = memoria();
    let (mut c, roteiro) = com_jogo_carregado(&memoria);

    c.quadro(&memoria);
    assert_eq!(c.eventos(), vec![]);

    memoria.bytes[1] = 3;
    memoria.bytes[2] = 7;
    c.quadro(&memoria);
    let eventos = c.eventos();
    let [Evento::Desbloqueou(conquista)] = &eventos[..] else {
        panic!("{eventos:?}");
    };
    assert_eq!(conquista.id, 5501);
    assert_eq!(conquista.titulo, "Primeira");
    assert_eq!(conquista.pontos, 5);

    let roteiro = roteiro.borrow();
    let premio = roteiro
        .pedidos
        .iter()
        .find(|p| parametro(p, "r").as_deref() == Some("awardachievement"))
        .expect("o desbloqueio foi enviado");
    assert_eq!(parametro(premio, "a").as_deref(), Some("5501"));
    assert_eq!(parametro(premio, "h").as_deref(), Some("0"));
}

/// Um endereço que a memória não tem desliga a conquista em vez de lê-la como zero. É a única
/// validação enquanto o Zeebo não tem mapa no `consoleinfo.c`.
#[test]
fn a_conquista_sobre_endereco_que_nao_se_le_nunca_dispara() {
    let memoria = memoria();
    let (mut c, roteiro) = com_jogo_carregado(&memoria);
    for _ in 0..10 {
        c.quadro(&memoria);
    }
    assert_eq!(c.eventos(), vec![]);
    assert!(
        !roteiro
            .borrow()
            .pedidos
            .iter()
            .any(|p| parametro(p, "a").as_deref() == Some("5502"))
    );
}

#[test]
fn o_hash_que_o_ra_nao_conhece_carrega_sem_conquistas() {
    let (mut c, _) = cliente(&[("login2", LOGIN), ("achievementsets", DESCONHECIDO)]);
    c.entra_com_senha("User", "Pa$$word");
    c.carrega_jogo("ffffffffffffffffffffffffffffffff");
    assert_eq!(
        c.eventos(),
        vec![
            Evento::Entrou(Ok(())),
            Evento::JogoCarregado(Err(ErroAoCarregar::Desconhecido))
        ]
    );
    assert!(c.jogo().is_none());
}

/// Sem memória, o carregamento espera: validar os endereços sem memória desligaria todas as
/// conquistas. O [`Evento::JogoCarregado`] só chega no primeiro quadro, que é quando o jogo é
/// ativado, e a conquista continua valendo.
#[test]
fn o_jogo_so_e_ativado_quando_ha_memoria() {
    let mut memoria = memoria();
    let (mut c, _) = cliente(&[
        ("login2", LOGIN),
        ("achievementsets", CONJUNTO),
        ("startsession", SEM_DESBLOQUEIOS),
        ("awardachievement", DESBLOQUEIO),
    ]);
    c.entra_com_senha("User", "Pa$$word");
    c.carrega_jogo("0123456789abcdef0123456789abcdef");
    c.ocioso(None);
    c.ocioso(None);
    assert_eq!(c.eventos(), vec![Evento::Entrou(Ok(()))]);
    assert!(c.jogo().is_none(), "ainda não ativado");

    c.quadro(&memoria);
    assert_eq!(c.eventos(), vec![Evento::JogoCarregado(Ok(()))]);

    // Um quadro com a condição falsa arma a conquista; só o seguinte pode dispará-la.
    c.quadro(&memoria);
    memoria.bytes[1] = 3;
    memoria.bytes[2] = 7;
    c.quadro(&memoria);
    let eventos = c.eventos();
    assert!(
        matches!(&eventos[..], [Evento::Desbloqueou(x)] if x.id == 5501),
        "{eventos:?}"
    );
}

/// O hash de referência do Zeebo é o MD5 do `.mod` inteiro, feito pelo `rc_hash` do fork.
#[test]
fn o_hash_do_modulo_e_o_md5_do_arquivo_inteiro() {
    assert_eq!(
        hash_do_modulo(b"abc").as_deref(),
        Some("900150983cd24fb0d6963f7d28e17f72")
    );
}

/// O RA não deixa entrar no hardcore no meio de uma partida: ligar com o jogo aberto pede que o
/// jogo recomece do zero.
#[test]
fn ligar_o_hardcore_com_o_jogo_aberto_pede_para_reiniciar() {
    let memoria = memoria();
    let (mut c, _) = com_jogo_carregado(&memoria);
    c.define_hardcore(true);
    assert!(c.hardcore());
    assert!(
        c.eventos().contains(&Evento::Reiniciar),
        "sem pedido de reinício"
    );
}

/// Sair do hardcore é livre, e não reinicia nada.
#[test]
fn sair_do_hardcore_nao_reinicia() {
    let memoria = memoria();
    let (mut c, _) = com_jogo_carregado(&memoria);
    c.define_hardcore(true);
    c.eventos();
    c.define_hardcore(false);
    assert!(!c.hardcore());
    assert!(!c.eventos().contains(&Evento::Reiniciar));
}

#[test]
fn o_hardcore_so_protege_o_jogo_que_tem_conquistas() {
    // Casual: nada a proteger.
    let memoria = memoria();
    let (mut c, _) = com_jogo_carregado(&memoria);
    assert!(!c.protege_o_jogo());
    // Hardcore com o jogo carregado: protege.
    c.define_hardcore(true);
    assert!(c.protege_o_jogo());

    // Hardcore sem jogo nenhum: nada a proteger.
    let (mut c, _) = cliente(&[("login2", LOGIN)]);
    c.define_hardcore(true);
    c.entra_com_senha("User", "Pa$$word");
    assert!(!c.protege_o_jogo());

    // Hardcore com um jogo que o RA não conhece: nada a proteger.
    let (mut c, _) = cliente(&[("login2", LOGIN), ("achievementsets", DESCONHECIDO)]);
    c.define_hardcore(true);
    c.entra_com_senha("User", "Pa$$word");
    c.carrega_jogo("ffffffffffffffffffffffffffffffff");
    assert!(!c.protege_o_jogo());
}

/// Enquanto o servidor não responde, o jogo ainda pode vir a ter conquistas: carregar um estado
/// nesse intervalo e só depois ver o set chegar seria entrar no hardcore com a partida mexida.
#[test]
fn o_hardcore_protege_o_jogo_que_ainda_esta_carregando() {
    let (mut c, roteiro) = cliente(&[("login2", LOGIN), ("achievementsets", CONJUNTO)]);
    c.define_hardcore(true);
    c.entra_com_senha("User", "Pa$$word");
    roteiro.borrow_mut().segura = true;
    c.carrega_jogo("0123456789abcdef0123456789abcdef");
    assert_eq!(
        roteiro.borrow().retidas.len(),
        1,
        "o pedido ficou esperando"
    );
    assert!(c.protege_o_jogo());

    // A resposta chega dizendo que o jogo é desconhecido: as regras se soltam.
    let (_, entrega) = roteiro.borrow_mut().retidas.pop().unwrap();
    entrega.responde(200, DESCONHECIDO.as_bytes().to_vec());
    c.ocioso(None);
    assert!(!c.protege_o_jogo());
}

/// O progresso guarda os contadores de acerto: uma conquista "três quadros" com dois já
/// contados dispara no quadro seguinte ao progresso voltar.
#[test]
fn o_progresso_guarda_os_acertos_contados() {
    let mut memoria = memoria();
    let mut c = jogo_com_a_condicao("0xH1001=1.3.", &memoria);
    memoria.bytes[1] = 1;
    c.quadro(&memoria);
    c.quadro(&memoria);
    let progresso = c.progresso();
    assert!(!progresso.is_empty());
    assert_eq!(c.eventos(), vec![]);

    // Recomeçar zera os acertos; o quadro com a condição falsa arma a conquista de novo.
    c.reinicia();
    memoria.bytes[1] = 0;
    c.quadro(&memoria);
    assert!(c.restaura_progresso(&progresso));
    memoria.bytes[1] = 1;
    c.quadro(&memoria);
    assert!(
        matches!(&c.eventos()[..], [Evento::Desbloqueou(x)] if x.id == 5501),
        "o terceiro acerto devia bastar"
    );
}

/// Um progresso vazio — o de um estado de antes das conquistas — zera os contadores.
#[test]
fn o_progresso_vazio_zera_os_acertos() {
    let mut memoria = memoria();
    let mut c = jogo_com_a_condicao("0xH1001=1.3.", &memoria);
    memoria.bytes[1] = 1;
    c.quadro(&memoria);
    c.quadro(&memoria);
    assert!(c.restaura_progresso(&[]));
    memoria.bytes[1] = 0;
    c.quadro(&memoria);
    memoria.bytes[1] = 1;
    c.quadro(&memoria);
    c.quadro(&memoria);
    assert_eq!(
        c.eventos(),
        vec![],
        "dois acertos depois de zerar não bastam"
    );
    c.quadro(&memoria);
    assert!(matches!(&c.eventos()[..], [Evento::Desbloqueou(x)] if x.id == 5501));
}

/// O servidor recebe o user agent completo — o do Zeebx e o do rcheevos — antes do primeiro
/// pedido: é ele que vai em cada requisição HTTP.
#[test]
fn o_servidor_recebe_o_user_agent_antes_de_pedir() {
    let (c, roteiro) = cliente(&[]);
    assert_eq!(roteiro.borrow().user_agent, c.user_agent());
}

#[test]
fn o_resumo_conta_as_oficiais_e_as_sem_suporte() {
    let memoria = memoria();
    let (c, _) = com_jogo_carregado(&memoria);
    assert_eq!(
        c.resumo(),
        Some(Resumo {
            total: 2,
            desbloqueadas: 0,
            sem_suporte: 1,
            pontos: 7,
            pontos_desbloqueados: 0,
        })
    );
}

/// A lista separa a que o emulador não lê das que valem, com os ícones do servidor de mídia; a
/// desbloqueada passa de grupo e fica marcada.
#[test]
fn a_lista_agrupa_as_conquistas_e_acompanha_o_desbloqueio() {
    let mut memoria = memoria();
    let (mut c, _) = com_jogo_carregado(&memoria);
    let lista = c.lista();
    let grupo = |lista: &[(Grupo, Vec<Conquista>)], id: u32| {
        lista
            .iter()
            .find(|(_, itens)| itens.iter().any(|x| x.id == id))
            .map(|(g, _)| *g)
    };
    assert_eq!(grupo(&lista, 5501), Some(Grupo::Bloqueadas), "{lista:?}");
    assert_eq!(grupo(&lista, 5502), Some(Grupo::SemSuporte), "{lista:?}");
    let primeira = lista
        .iter()
        .flat_map(|(_, i)| i)
        .find(|x| x.id == 5501)
        .unwrap();
    assert!(!primeira.desbloqueada && !primeira.sem_suporte);
    assert!(
        primeira
            .icone
            .as_deref()
            .is_some_and(|u| u.ends_with("00234.png")),
        "{primeira:?}"
    );
    assert!(
        primeira
            .icone_bloqueado
            .as_deref()
            .is_some_and(|u| u.ends_with("00234_lock.png")),
        "{primeira:?}"
    );

    c.quadro(&memoria);
    memoria.bytes[1] = 3;
    memoria.bytes[2] = 7;
    c.quadro(&memoria);
    let lista = c.lista();
    let primeira = lista
        .iter()
        .flat_map(|(_, i)| i)
        .find(|x| x.id == 5501)
        .unwrap();
    assert!(primeira.desbloqueada && !primeira.no_hardcore, "{lista:?}");
    assert_eq!(c.resumo().map(|r| r.desbloqueadas), Some(1));
}

/// O desbloqueio feito sem rede sobrevive ao fechamento: fica na fila em disco, e a execução
/// seguinte o manda de novo depois do login. O RA exige isso, e o rcheevos sozinho só guarda na
/// memória.
///
/// A segunda execução fala com **outro** servidor, de propósito: a primeira versão da fila
/// guardava a URL, e na janela de verdade mandou o desbloqueio para o host da execução anterior.
#[test]
fn o_desbloqueio_sem_rede_vai_na_execucao_seguinte() {
    let pasta = crate::scratch::TempDir::new("zeebx-conquistas-fila-offline");
    let caminho = pasta.join("pendentes.json");

    // Primeira execução: o desbloqueio acontece, e o servidor não responde.
    let mut memoria = memoria();
    let roteiro = Rc::new(RefCell::new(Roteiro {
        respostas: [
            ("login2", LOGIN.to_string()),
            ("achievementsets", CONJUNTO.to_string()),
            ("startsession", SEM_DESBLOQUEIOS.to_string()),
        ]
        .into_iter()
        .collect(),
        sem_rede: vec!["awardachievement"],
        ..Default::default()
    }));
    let mut c = Conquistas::com_fila(
        Box::new(ServidorSimulado(Rc::clone(&roteiro))),
        pendentes::Pendentes::em(caminho.clone()),
    );
    c.entra_com_senha("User", "Pa$$word");
    c.carrega_jogo("0123456789abcdef0123456789abcdef");
    c.quadro(&memoria);
    c.quadro(&memoria);
    memoria.bytes[1] = 3;
    memoria.bytes[2] = 7;
    c.quadro(&memoria);
    assert!(
        c.eventos()
            .iter()
            .any(|e| matches!(e, Evento::Desbloqueou(x) if x.id == 5501))
    );
    assert_eq!(c.pendentes(), 1, "o desbloqueio sem resposta ficou na fila");
    drop(c);

    // Dois minutos sem rede: o desbloqueio aconteceu 120 s antes da execução seguinte.
    let mut fila: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&caminho).unwrap()).unwrap();
    let quando = fila[0]["quando"].as_u64().unwrap();
    fila[0]["quando"] = serde_json::json!(quando - 120);
    std::fs::write(&caminho, serde_json::to_vec(&fila).unwrap()).unwrap();

    // Segunda execução, com rede: o login manda o que ficou, e a fila esvazia.
    let roteiro = Rc::new(RefCell::new(Roteiro {
        respostas: [
            ("login2", LOGIN.to_string()),
            ("awardachievement", DESBLOQUEIO.to_string()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    }));
    let mut c = Conquistas::com_fila(
        Box::new(ServidorSimulado(Rc::clone(&roteiro))),
        pendentes::Pendentes::em(caminho.clone()),
    );
    c.define_host(Some("http://outro.servidor"));
    assert_eq!(c.pendentes(), 1);
    c.entra_com_senha("User", "Pa$$word");
    c.ocioso(None);
    let reenviado = roteiro
        .borrow()
        .pedidos
        .iter()
        .find(|p| parametro(p, "r").as_deref() == Some("awardachievement"))
        .cloned()
        .expect("o desbloqueio foi mandado de novo");
    assert_eq!(parametro(&reenviado, "a").as_deref(), Some("5501"));
    assert_eq!(parametro(&reenviado, "h").as_deref(), Some("0"));
    assert_eq!(
        parametro(&reenviado, "t").as_deref(),
        Some("ApiToken"),
        "o token do login de agora"
    );
    let atras: u64 = parametro(&reenviado, "o")
        .and_then(|o| o.parse().ok())
        .expect("há quanto tempo foi");
    assert!(
        (120..130).contains(&atras),
        "o={atras}: o instante é o do desbloqueio"
    );
    assert!(
        reenviado.url.starts_with("http://outro.servidor/"),
        "o servidor de agora: {}",
        reenviado.url
    );
    assert_eq!(c.pendentes(), 0, "a resposta tirou da fila");
    drop(c);
    assert_eq!(
        pendentes::Pendentes::em(caminho).quantos(),
        0,
        "o arquivo também"
    );
}
