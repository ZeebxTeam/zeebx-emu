//! Janela do host, para acompanhar o jogo rodando.
//!
//! É a única parte do emulador que fala com o sistema de janelas, e por isso a dependência
//! fica confinada aqui: o resto do código só entrega um [`Framebuffer`] e pergunta se a janela
//! continua aberta.

use minifb::{Key, KeyRepeat, Scale, WindowOptions};

use crate::input::{self, Pad};
use crate::video::display::Framebuffer;

pub struct Window {
    inner: minifb::Window,
    width: usize,
    height: usize,
}

impl Window {
    /// Abre uma janela do tamanho da tela do console.
    pub fn open(title: &str, width: u32, height: u32) -> Result<Self, String> {
        let (width, height) = (width as usize, height as usize);
        let mut inner = minifb::Window::new(
            title,
            width,
            height,
            WindowOptions {
                scale: Scale::X1,
                resize: true,
                ..WindowOptions::default()
            },
        )
        .map_err(|err| err.to_string())?;
        // Sem limite: quem dita o ritmo é o emulador, e segurar aqui só o deixaria mais lento.
        inner.set_target_fps(0);
        Ok(Self {
            inner,
            width,
            height,
        })
    }

    /// Se a janela continua aberta. `Esc` fecha, como em qualquer emulador.
    pub fn is_open(&self) -> bool {
        self.inner.is_open() && !self.inner.is_key_down(Key::Escape)
    }

    /// O estado do controle, lido do teclado.
    ///
    /// As setas movem os eixos `X` e `Y`, que é onde o controle do Zeebo reporta o direcional;
    /// o resto são botões. Os nomes dos botões são os do controle, não os das teclas — a
    /// tabela de quem é quem está em [`crate::input`].
    pub fn pad(&self) -> Pad {
        let mut pad = Pad::default();
        // Uma tecla que foi apertada e solta entre dois quadros não apareceria em
        // `is_key_down`, e o emulador só olha o teclado uma vez por quadro: sem os eventos de
        // borda, um toque rápido se perdia.
        let tapped = self.inner.get_keys_pressed(KeyRepeat::No);
        let held = |key| self.inner.is_key_down(key) || tapped.contains(&key);

        let arrows = [
            (Key::Up, input::DPAD[0]),
            (Key::Down, input::DPAD[1]),
            (Key::Left, input::DPAD[2]),
            (Key::Right, input::DPAD[3]),
        ];
        for (key, index) in arrows {
            pad.press(index, held(key));
        }
        for (key, button) in KEYMAP {
            if held(key) {
                if let Some(index) = Pad::button_by_name(button) {
                    pad.press(index, true);
                }
            }
        }
        pad
    }

    /// Mostra o quadro.
    pub fn show(&mut self, frame: &Framebuffer) -> Result<(), String> {
        let pixels = frame.to_argb();
        // A tela do guest pode mudar de tamanho entre um quadro e outro; a janela não.
        if pixels.len() != self.width * self.height {
            return Ok(());
        }
        self.inner
            .update_with_buffer(&pixels, self.width, self.height)
            .map_err(|err| err.to_string())
    }
}

/// De qual tecla vem cada botão do controle.
///
/// A escolha é a de qualquer emulador: a mão esquerda nas setas e a direita nas teclas de
/// ação, com `Enter` e `Backspace` no `Start` e no `Back`.
const KEYMAP: [(Key, &str); 12] = [
    (Key::Z, "b1"),
    (Key::Space, "b2"),
    (Key::X, "b2"),
    (Key::C, "b3"),
    (Key::V, "b4"),
    (Key::Q, "zl"),
    (Key::W, "zr"),
    (Key::F, "lthumb"),
    (Key::G, "rthumb"),
    // O controle do Zeebo não tem Start: o HOME faz esse papel.
    (Key::Enter, "back"),
    (Key::Backspace, "back"),
    (Key::H, "home"),
];
