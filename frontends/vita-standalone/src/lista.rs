//! A lista de jogos: o que um aperto faz, e onde o cursor fica.
//!
//! Separado da leitura do controle para dar para provar sem o aparelho. A repetição do
//! direcional existe porque uma pasta com dezenas de jogos não se percorre um toque por vez.

use std::time::Duration;

use crate::entrada;

pub enum Toque {
    Nada,
    Confirma,
    Sai,
    Atualiza,
}

/// Quanto segurar o direcional antes de ele começar a repetir, e o intervalo da repetição.
pub const ESPERA_DA_REPETICAO: Duration = Duration::from_millis(280);
pub const PASSO_DA_REPETICAO: Duration = Duration::from_millis(90);

pub fn toque(agora: u32, antes: u32) -> Toque {
    let subiu = |bit: u32| agora & bit != 0 && antes & bit == 0;
    if subiu(entrada::CRUZ) {
        Toque::Confirma
    } else if subiu(entrada::CIRCULO) {
        Toque::Sai
    } else if subiu(entrada::TRIANGULO) {
        Toque::Atualiza
    } else {
        Toque::Nada
    }
}

/// `-1` é cima, `1` é baixo. Os dois juntos não são direção: é o manche encostado no canto, ou
/// as duas teclas, e andar nos dois sentidos no mesmo quadro não tem destino.
pub fn direcao(botoes: u32) -> i32 {
    match (botoes & entrada::CIMA != 0, botoes & entrada::BAIXO != 0) {
        (true, false) => -1,
        (false, true) => 1,
        _ => 0,
    }
}

/// A borda entra na hora. Segurar só repete depois da espera, e daí no intervalo.
pub fn passo_da_lista(borda: i32, segurando: i32, ha: Duration, desde_o_ultimo: Duration) -> i32 {
    if borda != 0 {
        return borda;
    }
    if segurando != 0 && ha >= ESPERA_DA_REPETICAO && desde_o_ultimo >= PASSO_DA_REPETICAO {
        return segurando;
    }
    0
}

pub fn move_cursor(cursor: usize, total: usize, delta: i32) -> usize {
    if total == 0 {
        return 0;
    }
    let atual = cursor.min(total - 1) as i32;
    (atual + delta).clamp(0, total as i32 - 1) as usize
}

/// A primeira linha visível, com o cursor sempre dentro da janela e grudado na última linha
/// quando a lista é maior que a tela.
pub fn janela(cursor: usize, total: usize, visiveis: usize) -> usize {
    if visiveis == 0 || total <= visiveis {
        return 0;
    }
    let cursor = cursor.min(total - 1);
    cursor.saturating_sub(visiveis - 1).min(total - visiveis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_aperto_vale_uma_vez() {
        assert!(matches!(toque(entrada::CRUZ, 0), Toque::Confirma));
        assert!(matches!(toque(entrada::CRUZ, entrada::CRUZ), Toque::Nada));
        assert!(matches!(toque(entrada::CIRCULO, 0), Toque::Sai));
        assert!(matches!(toque(entrada::TRIANGULO, 0), Toque::Atualiza));
    }

    #[test]
    fn o_direcional_repete_so_depois_da_espera() {
        assert_eq!(direcao(entrada::CIMA), -1);
        assert_eq!(direcao(entrada::CIMA | entrada::BAIXO), 0);
        assert_eq!(passo_da_lista(1, 1, Duration::ZERO, Duration::ZERO), 1);
        assert_eq!(
            passo_da_lista(0, 1, Duration::from_millis(100), Duration::from_millis(100)),
            0
        );
        assert_eq!(
            passo_da_lista(0, 1, ESPERA_DA_REPETICAO, PASSO_DA_REPETICAO),
            1
        );
        assert_eq!(
            passo_da_lista(0, 0, Duration::from_secs(2), Duration::from_secs(2)),
            0
        );
    }

    #[test]
    fn o_cursor_nao_sai_da_lista_e_a_janela_acompanha() {
        assert_eq!(move_cursor(0, 0, 1), 0);
        assert_eq!(move_cursor(0, 3, -1), 0);
        assert_eq!(move_cursor(0, 3, 1), 1);
        assert_eq!(move_cursor(2, 3, 1), 2);
        assert_eq!(janela(0, 20, 10), 0);
        assert_eq!(janela(10, 20, 10), 1);
        assert_eq!(janela(19, 20, 10), 10);
        assert_eq!(janela(3, 4, 10), 0);
    }
}
