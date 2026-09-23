//! O quadro saindo sem janela.
//!
//! É o modo para quem já tem onde pintar: um frontend que simula o console desenha a tela dele
//! e quer só os pixels. Aqui não há janela, não há teclado — quem joga usa um controle, que o
//! sistema entrega sem precisar de foco — e o 3D, quando é pedido na placa, roda no contexto
//! fora de tela do núcleo.
//!
//! O formato é fixo e sem cabeçalho: um quadro atrás do outro, sempre do mesmo tamanho. Quem lê
//! já sabe o tamanho — o console tem um só — e um cabeçalho por quadro seria uma coisa a mais
//! para combinar entre dois programas que já combinaram tudo no `config.ini`.

use std::io::Write;
use std::path::Path;

use zeebx::video::display::Framebuffer;

use crate::config::{Despejo, Formato};

/// Para onde os quadros vão.
pub struct Saida {
    formato: Formato,
    destino: Destino,
    /// Quantos já saíram. É o que numera os `.png` e o que o limite conta.
    contados: u64,
    limite: u64,
}

enum Destino {
    /// A saída padrão. É o que um `zeebx-headless jogo.mod | meu-frontend` usa.
    Padrao,
    /// Um arquivo, que pode ser um FIFO — e aí não passa por disco.
    Arquivo(std::fs::File),
    /// Uma pasta com um `.png` por quadro.
    Pasta(std::path::PathBuf),
}

impl Saida {
    pub fn abre(despejo: &Despejo) -> Result<Self, String> {
        let destino = match (&despejo.destino, despejo.formato) {
            (Some(caminho), Formato::Png) => {
                std::fs::create_dir_all(caminho)
                    .map_err(|erro| format!("could not create {}: {erro}", caminho.display()))?;
                Destino::Pasta(caminho.clone())
            }
            // PNG na saída padrão seria um arquivo atrás do outro no mesmo fluxo, que nenhum
            // leitor de PNG desembrulha. Dizer isso é melhor que escrever algo inútil.
            (None, Formato::Png) => {
                return Err("png dumping needs a folder in `destination`".to_string());
            }
            (Some(caminho), _) => {
                let arquivo = abre_para_escrita(caminho)
                    .map_err(|erro| format!("could not open {}: {erro}", caminho.display()))?;
                Destino::Arquivo(arquivo)
            }
            (None, _) => Destino::Padrao,
        };
        Ok(Self {
            formato: despejo.formato,
            destino,
            contados: 0,
            limite: despejo.limite,
        })
    }

    /// Se já saíram quadros demais. É o que encerra a execução no modo sem janela.
    pub fn cheia(&self) -> bool {
        self.limite > 0 && self.contados >= self.limite
    }

    pub fn contados(&self) -> u64 {
        self.contados
    }

    /// Escreve um quadro.
    ///
    /// Um erro aqui **encerra**, e não é ignorado: a outra ponta fechou o cano, e continuar
    /// emulando para escrever em coisa nenhuma só gastaria a máquina de quem rodou.
    pub fn escreve(&mut self, quadro: &Framebuffer) -> Result<(), String> {
        let (largura, altura) = (quadro.width(), quadro.height());
        match self.formato {
            Formato::Rgb565 => self.bytes(&quadro.to_rgb565_bytes())?,
            Formato::Rgba => {
                let rgba: Vec<u8> = quadro
                    .to_argb()
                    .into_iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                    .collect();
                self.bytes(&rgba)?;
            }
            Formato::Png => {
                let Destino::Pasta(pasta) = &self.destino else {
                    return Err("png dumping without a folder".to_string());
                };
                let caminho = pasta.join(format!("quadro-{:06}.png", self.contados));
                let rgba: Vec<u8> = quadro
                    .to_argb()
                    .into_iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                    .collect();
                grava_png(&caminho, &rgba, largura, altura)
                    .map_err(|erro| format!("could not write {}: {erro}", caminho.display()))?;
            }
        }
        self.contados += 1;
        Ok(())
    }

    fn bytes(&mut self, dados: &[u8]) -> Result<(), String> {
        let escrito = match &mut self.destino {
            Destino::Padrao => {
                let saida = std::io::stdout();
                let mut travada = saida.lock();
                // Cada quadro sai inteiro ou não sai: quem lê conta bytes, e meio quadro
                // desalinharia tudo o que vier depois.
                travada.write_all(dados).and_then(|()| travada.flush())
            }
            Destino::Arquivo(arquivo) => arquivo.write_all(dados).and_then(|()| arquivo.flush()),
            Destino::Pasta(_) => Ok(()),
        };
        escrito.map_err(|erro| format!("the frame did not go out: {erro}"))
    }
}

/// Abre para escrita sem truncar o que não é arquivo comum.
///
/// Um FIFO é o caso que importa: `File::create` nele bloqueia até alguém abrir a outra ponta,
/// o que é justamente o que se quer — mas truncar não faz sentido, e em alguns sistemas dá
/// erro. Por isso as opções são ditas à mão.
fn abre_para_escrita(caminho: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(caminho.is_file())
        .open(caminho)
}

fn grava_png(caminho: &Path, rgba: &[u8], largura: u32, altura: u32) -> std::io::Result<()> {
    let arquivo = std::fs::File::create(caminho)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(arquivo), largura, altura);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut escritor = encoder.write_header().map_err(std::io::Error::other)?;
    escritor.write_image_data(rgba).map_err(std::io::Error::other)
}
