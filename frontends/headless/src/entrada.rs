//! O teclado do winit e os controles do host virando o estado das portas do console.
//!
//! O mapeamento em si é o do núcleo — [`Player::pad`] — e os nomes de tecla são os do egui, os
//! mesmos que a tela de controles do desktop grava no `settings.json`. **Isso é de propósito:**
//! um mapeamento copiado da interface para o `config.ini` tem de valer sem tradução, senão o
//! jeito mais fácil de descobrir como um botão se chama deixaria de servir.
//!
//! Aqui não há egui desenhando nada: o `egui::Key` é usado só como o nome canônico de uma
//! tecla, que é o que ele já era no arquivo de configuração.

use std::collections::HashSet;

use winit::keyboard::{KeyCode, PhysicalKey};

use zeebx::input::bindings::{Player, Source};
use zeebx::input::{Pad, gamepads::Gamepads};

/// O teclado do host: que teclas estão apertadas agora.
#[derive(Default)]
pub struct Teclado {
    apertadas: HashSet<egui::Key>,
}

impl Teclado {
    /// Anota o que o winit contou. `None` quando a tecla não é uma que se saiba nomear — um
    /// `F13`, uma tecla de mídia —, e aí não há o que anotar.
    pub fn evento(&mut self, tecla: PhysicalKey, apertada: bool) {
        let PhysicalKey::Code(codigo) = tecla else {
            return;
        };
        let Some(key) = do_winit(codigo) else {
            return;
        };
        match apertada {
            true => self.apertadas.insert(key),
            false => self.apertadas.remove(&key),
        };
    }

    /// A janela perdeu o foco: o que estava apertado não está mais.
    ///
    /// Sem isto, sair da janela com uma tecla apertada deixa o jogo achando que ela continua —
    /// o `keyup` acontece noutra janela e nunca chega aqui.
    pub fn solta_tudo(&mut self) {
        self.apertadas.clear();
    }

    pub fn apertadas(&self) -> &HashSet<egui::Key> {
        &self.apertadas
    }

    fn acionada(&self, source: &Source) -> bool {
        match source {
            Source::Key { name } => {
                egui::Key::from_name(name).is_some_and(|key| self.apertadas.contains(&key))
            }
            _ => false,
        }
    }
}

/// O estado do controle de uma porta, do mapeamento dela.
///
/// Teclado e controle são consultados **juntos**: quem tem os dois pode usar os dois, que é o
/// que ter mais de uma origem por botão significa.
pub fn pad_da_porta(player: &Player, teclado: &Teclado, pads: &Gamepads, porta: usize) -> Pad {
    let device = player.device.as_deref();
    player.pad(
        |source| teclado.acionada(source) || pads.is_active(device, porta, source),
        |axis| pads.value(device, porta, axis),
    )
}

/// A tecla do winit pelo nome que o egui — e portanto o `config.ini` — dá a ela.
///
/// É por código **físico**, não por caractere: o `Z` da configuração é a tecla onde o `Z` fica
/// num teclado americano, que é o que todo emulador faz. Assim um teclado ABNT2 ou AZERTY joga
/// com a mesma configuração, com a mão no mesmo lugar.
fn do_winit(codigo: KeyCode) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match codigo {
        KeyCode::ArrowUp => K::ArrowUp,
        KeyCode::ArrowDown => K::ArrowDown,
        KeyCode::ArrowLeft => K::ArrowLeft,
        KeyCode::ArrowRight => K::ArrowRight,

        KeyCode::Escape => K::Escape,
        KeyCode::Tab => K::Tab,
        KeyCode::Backspace => K::Backspace,
        KeyCode::Enter | KeyCode::NumpadEnter => K::Enter,
        KeyCode::Space => K::Space,
        KeyCode::Insert => K::Insert,
        KeyCode::Delete => K::Delete,
        KeyCode::Home => K::Home,
        KeyCode::End => K::End,
        KeyCode::PageUp => K::PageUp,
        KeyCode::PageDown => K::PageDown,

        KeyCode::Minus | KeyCode::NumpadSubtract => K::Minus,
        KeyCode::Equal => K::Equals,
        KeyCode::NumpadAdd => K::Plus,
        KeyCode::Comma | KeyCode::NumpadComma => K::Comma,
        KeyCode::Period | KeyCode::NumpadDecimal => K::Period,
        KeyCode::Semicolon => K::Semicolon,
        KeyCode::Quote => K::Quote,
        KeyCode::Backquote => K::Backtick,
        KeyCode::Backslash => K::Backslash,
        KeyCode::Slash | KeyCode::NumpadDivide => K::Slash,
        KeyCode::BracketLeft => K::OpenBracket,
        KeyCode::BracketRight => K::CloseBracket,

        KeyCode::Digit0 | KeyCode::Numpad0 => K::Num0,
        KeyCode::Digit1 | KeyCode::Numpad1 => K::Num1,
        KeyCode::Digit2 | KeyCode::Numpad2 => K::Num2,
        KeyCode::Digit3 | KeyCode::Numpad3 => K::Num3,
        KeyCode::Digit4 | KeyCode::Numpad4 => K::Num4,
        KeyCode::Digit5 | KeyCode::Numpad5 => K::Num5,
        KeyCode::Digit6 | KeyCode::Numpad6 => K::Num6,
        KeyCode::Digit7 | KeyCode::Numpad7 => K::Num7,
        KeyCode::Digit8 | KeyCode::Numpad8 => K::Num8,
        KeyCode::Digit9 | KeyCode::Numpad9 => K::Num9,

        KeyCode::KeyA => K::A,
        KeyCode::KeyB => K::B,
        KeyCode::KeyC => K::C,
        KeyCode::KeyD => K::D,
        KeyCode::KeyE => K::E,
        KeyCode::KeyF => K::F,
        KeyCode::KeyG => K::G,
        KeyCode::KeyH => K::H,
        KeyCode::KeyI => K::I,
        KeyCode::KeyJ => K::J,
        KeyCode::KeyK => K::K,
        KeyCode::KeyL => K::L,
        KeyCode::KeyM => K::M,
        KeyCode::KeyN => K::N,
        KeyCode::KeyO => K::O,
        KeyCode::KeyP => K::P,
        KeyCode::KeyQ => K::Q,
        KeyCode::KeyR => K::R,
        KeyCode::KeyS => K::S,
        KeyCode::KeyT => K::T,
        KeyCode::KeyU => K::U,
        KeyCode::KeyV => K::V,
        KeyCode::KeyW => K::W,
        KeyCode::KeyX => K::X,
        KeyCode::KeyY => K::Y,
        KeyCode::KeyZ => K::Z,

        KeyCode::F1 => K::F1,
        KeyCode::F2 => K::F2,
        KeyCode::F3 => K::F3,
        KeyCode::F4 => K::F4,
        KeyCode::F5 => K::F5,
        KeyCode::F6 => K::F6,
        KeyCode::F7 => K::F7,
        KeyCode::F8 => K::F8,
        KeyCode::F9 => K::F9,
        KeyCode::F10 => K::F10,
        KeyCode::F11 => K::F11,
        KeyCode::F12 => K::F12,

        _ => return None,
    })
}

#[cfg(test)]
mod testes {
    use super::*;

    /// O nome que o `config.ini` usa precisa achar a tecla que o winit entrega. Se esta tabela
    /// e a do egui discordarem, `tecla:Z` vira uma linha que não faz nada.
    #[test]
    fn os_nomes_do_arquivo_batem_com_as_teclas_do_winit() {
        let mut teclado = Teclado::default();
        for (codigo, nome) in [
            (KeyCode::KeyZ, "Z"),
            (KeyCode::Space, "Space"),
            (KeyCode::ArrowUp, "ArrowUp"),
            (KeyCode::Enter, "Enter"),
            (KeyCode::Digit1, "1"),
            (KeyCode::Backspace, "Backspace"),
        ] {
            teclado.evento(PhysicalKey::Code(codigo), true);
            assert!(
                teclado.acionada(&Source::key(nome)),
                "`tecla:{nome}` não vê {codigo:?}"
            );
            teclado.evento(PhysicalKey::Code(codigo), false);
            assert!(!teclado.acionada(&Source::key(nome)), "{nome} ficou presa");
        }
    }

    #[test]
    fn perder_o_foco_solta_o_que_estava_apertado() {
        let mut teclado = Teclado::default();
        teclado.evento(PhysicalKey::Code(KeyCode::KeyZ), true);
        teclado.solta_tudo();
        assert!(!teclado.acionada(&Source::key("Z")));
    }
}
