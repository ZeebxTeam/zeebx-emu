//! O teste de ponta a ponta: login, jogo, quadros pelo relógio virtual, desbloqueio e save state,
//! contra um servidor simulado. O JSON é o que o servidor do RA devolve, copiado dos testes do
//! próprio rcheevos (`test/client/test_rc_client.c`), com o console trocado para o Zeebo.

use super::*;
use crate::cpu::CpuBackend;

/// Responde pelo valor de `r=` e guarda os pedidos.
struct ServidorSimulado(Rc<RefCell<Vec<Pedido>>>);

impl Servidor for ServidorSimulado {
    fn pede(&mut self, pedido: Pedido, entrega: Entrega) {
        let corpo = pedido.corpo.clone().unwrap_or_default();
        let r = corpo
            .split('&')
            .find_map(|par| par.strip_prefix("r="))
            .unwrap_or_default();
        let resposta = match r {
            "login2" => {
                "{\"Success\":true,\"User\":\"User\",\"Token\":\"ApiToken\",\"Score\":12345,\
                \"SoftcoreScore\":123,\"Messages\":0,\"Permissions\":1,\"AccountType\":\"Registered\"}"
            }
            // A 5501 quer o byte 0x1001 igual a 1 em três quadros.
            "achievementsets" => {
                "{\"Success\":true,\"GameId\":1234,\"Title\":\"Jogo de Teste\",\
                \"ConsoleId\":70,\"ImageIconUrl\":\"http://server/Images/112233.png\",\
                \"RichPresenceGameId\":1234,\"RichPresencePatch\":\"\",\"Sets\":[{\
                \"AchievementSetId\":1111,\"GameId\":1234,\"Title\":null,\"Type\":\"core\",\
                \"ImageIconUrl\":\"http://server/Images/112233.png\",\"Achievements\":[\
                {\"ID\":5501,\"Title\":\"Primeira\",\"Description\":\"Três vezes\",\"Flags\":3,\
                \"Points\":5,\"MemAddr\":\"0xH1001=1.3.\",\"Author\":\"User1\",\
                \"BadgeName\":\"00234\",\"Created\":1367266583,\"Modified\":1376929305}],\
                \"Leaderboards\":[]}]}"
            }
            "startsession" => "{\"Success\":true,\"Unlocks\":[],\"HardcoreUnlocks\":[]}",
            "awardachievement" => {
                "{\"Success\":true,\"Score\":5432,\"SoftcoreScore\":128,\
                \"AchievementID\":5501,\"AchievementsRemaining\":0}"
            }
            _ => "{\"Success\":false,\"Error\":\"sem resposta simulada\"}",
        };
        self.0.borrow_mut().push(pedido);
        entrega.responde(200, resposta.as_bytes().to_vec());
    }
}

#[test]
fn uma_conquista_sai_de_ponta_a_ponta() {
    const QUADRO_US: u64 = 1_000_000 / 60 + 1;
    let pedidos = Rc::new(RefCell::new(Vec::new()));
    let cliente = Rc::new(RefCell::new(Conquistas::novo(Box::new(ServidorSimulado(
        Rc::clone(&pedidos),
    )))));
    assert!(cliente.borrow().user_agent().starts_with("Zeebx/"));
    cliente.borrow_mut().entra_com_senha("User", "Pa$$word");
    assert_eq!(cliente.borrow_mut().eventos(), vec![Evento::Entrou(Ok(()))]);
    assert_eq!(
        cliente.borrow().usuario().map(|u| u.token),
        Some("ApiToken".into())
    );

    let mut sessao = crate::session::tests::sessao_minima_para_save_state();
    sessao.conquistas.hash = Some("0123456789abcdef0123456789abcdef".into());
    sessao.liga_conquistas(Rc::clone(&cliente));
    let quadro = |sessao: &mut crate::session::Session, byte: u8| {
        sessao
            .machine_mut()
            .cpu_mut()
            .write_mem(0x1001, &[byte])
            .unwrap();
        sessao.machine_mut().adianta_o_relogio_us(QUADRO_US);
        sessao.acompanha_as_conquistas();
        cliente.borrow_mut().eventos()
    };

    // O primeiro acompanhamento só ancora. Sem o relógio andar, nenhum quadro é avaliado.
    sessao.acompanha_as_conquistas();
    sessao.acompanha_as_conquistas();
    assert_eq!(cliente.borrow_mut().eventos(), vec![]);
    // O jogo é ativado no primeiro quadro, e a conquista arma vendo a condição falsa.
    let jogo = Jogo {
        titulo: "Jogo de Teste".into(),
        icone: Some("http://server/Images/112233.png".into()),
        total: 1,
        desbloqueadas: 0,
    };
    assert_eq!(
        quadro(&mut sessao, 0),
        vec![Evento::JogoCarregado(Ok(Some(jogo)))]
    );
    assert_eq!(quadro(&mut sessao, 0), vec![]);
    assert_eq!(quadro(&mut sessao, 1), vec![]);
    assert_eq!(quadro(&mut sessao, 1), vec![]);

    // O estado leva os dois acertos. Zerados, voltar a ele deixa o terceiro bastar.
    let estado = sessao.grava_estado();
    cliente.borrow_mut().restaura_progresso(&[]);
    sessao.restaura_estado(&estado).expect("o estado carrega");
    sessao.acompanha_as_conquistas(); // só ancora: o relógio foi para onde o estado mandou
    let eventos = quadro(&mut sessao, 1);
    assert!(
        matches!(&eventos[..], [Evento::Desbloqueou(c)] if c.id == 5501 && c.pontos == 5),
        "{eventos:?}"
    );

    let award = pedidos
        .borrow()
        .iter()
        .filter_map(|p| p.corpo.clone())
        .find(|c| c.starts_with("r=awardachievement"));
    let award = award.expect("o desbloqueio foi enviado");
    assert!(award.contains("a=5501") && award.contains("h=0"), "{award}");
}
