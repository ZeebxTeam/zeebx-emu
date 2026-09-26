//! O Rich Presence do Discord: o que o emulador está fazendo, no perfil de quem joga.
//!
//! O Discord conversa por um soquete local (`discord-ipc-N`), e a conexão pode faltar a qualquer
//! hora — o Discord fechado, aberto depois do emulador, reiniciado. Por isso quem fala com ele é
//! uma thread própria: a interface só diz **o que** mostrar, e a thread conecta, reconecta e
//! reenvia sem travar um quadro sequer.
//!
//! As imagens não vão pelo soquete. O Discord aceita uma **chave** de imagem cadastrada no
//! aplicativo (Developer Portal → Rich Presence → Art Assets) ou uma URL `https`. A capa de um
//! jogo sai da ROM e mora só no disco, então ela entra de um desses dois jeitos: exportada e
//! cadastrada com a chave [`chave_da_capa`], ou publicada num endereço dado nas configurações.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::video::icon::Image;

/// O aplicativo Zeebx no Developer Portal. É ele que dá à presença o nome e as imagens.
pub const APLICATIVO: &str = "1548822136712466476";

/// A chave do ícone do emulador entre as imagens do aplicativo.
pub const CHAVE_DO_ICONE: &str = "zeebx";

/// De quanto em quanto tempo tentar de novo quando o Discord não está aberto.
const NOVA_TENTATIVA: Duration = Duration::from_secs(15);

/// O lado de uma capa exportada. O Developer Portal recusa arte com menos de 512×512.
pub const LADO_DA_CAPA: usize = 512;

/// A chave com que a capa de um jogo é cadastrada no aplicativo.
///
/// O Discord só aceita minúsculas, dígitos e sublinhado nas chaves, e o ClassID é a única coisa
/// do jogo que não muda de uma ROM para outra.
pub fn chave_da_capa(clsid: u32) -> String {
    format!("jogo_{clsid:08x}")
}

/// O que mostrar no perfil.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atividade {
    /// A linha de cima: "Jogando Bóia Cross", "No menu".
    pub detalhes: String,
    /// A imagem grande: chave cadastrada ou URL.
    pub imagem: String,
    pub texto_da_imagem: String,
    /// O ícone pequeno no canto da imagem grande, com o texto dele.
    pub icone: Option<(String, String)>,
    /// Desde quando, em milissegundos Unix. O Discord conta o tempo a partir daqui.
    pub inicio_ms: i64,
}

/// A presença acompanhando o que a interface mostra: o menu, a Z-Wheel ou o jogo aberto.
///
/// Saiu do `App` do egui para a interface Qt mostrar o mesmo no Discord. Guarda desde quando a
/// atividade atual começou — o relógio do perfil recomeça quando o jogo muda, e só aí.
pub struct Acompanha {
    presenca: Presenca,
    /// O ClassID do jogo aberto, ou nenhum no menu, e o instante em milissegundos Unix.
    inicio: (Option<u32>, i64),
}

impl Default for Acompanha {
    fn default() -> Self {
        Self {
            presenca: Presenca::default(),
            inicio: (None, agora_ms()),
        }
    }
}

impl Acompanha {
    /// Diz ao Discord o que está acontecendo. Barato de chamar a cada quadro: a presença só manda
    /// alguma coisa quando o texto ou a imagem mudam.
    ///
    /// `classe` é o ClassID do jogo aberto; `titulo`, o nome com que a biblioteca o mostra.
    pub fn atualiza(
        &mut self,
        ativo: bool,
        catalogo: &crate::ui::i18n::Catalog,
        classe: Option<u32>,
        titulo: Option<String>,
        capas_url: &str,
    ) {
        if self.inicio.0 != classe {
            self.inicio = (classe, agora_ms());
        }
        let atividade = ativo.then(|| atividade(catalogo, classe, titulo, capas_url, self.inicio.1));
        self.presenca.define(atividade);
    }

    /// Se o Discord respondeu e a presença está sendo mostrada.
    pub fn conectado(&self) -> bool {
        self.presenca.conectado()
    }
}

/// O que mostrar no perfil: no menu, na Z-Wheel, ou jogando `titulo`.
fn atividade(
    catalogo: &crate::ui::i18n::Catalog,
    classe: Option<u32>,
    titulo: Option<String>,
    capas_url: &str,
    inicio_ms: i64,
) -> Atividade {
    let icone = CHAVE_DO_ICONE.to_string();
    let menu = |chave: &str| Atividade {
        detalhes: catalogo.get(chave).to_string(),
        imagem: icone.clone(),
        texto_da_imagem: "Zeebx".to_string(),
        icone: None,
        inicio_ms,
    };
    let Some(classe) = classe else {
        return menu("discord.menu");
    };
    if classe == crate::session::Z_WHEEL {
        return menu("discord.z_wheel");
    }
    let titulo = titulo.unwrap_or_else(|| catalogo.get("library.unknown_title").to_string());
    let chave = chave_da_capa(classe);
    let modelo = capas_url.trim();
    let imagem = match modelo.is_empty() {
        true => chave,
        false => modelo
            .replace("{clsid}", &format!("{classe:08x}"))
            .replace("{chave}", &chave),
    };
    Atividade {
        detalhes: catalogo.format("discord.playing", &[("name", &titulo)]),
        imagem,
        texto_da_imagem: titulo,
        icone: Some((icone, "Zeebx".to_string())),
        inicio_ms,
    }
}

/// O instante atual em milissegundos Unix, que é o que o Discord quer no início da atividade.
pub fn agora_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

enum Pedido {
    Mostra(String, Option<Atividade>),
    Encerra,
}

/// A presença, do lado da interface. Só manda pedidos quando algo muda.
pub struct Presenca {
    envio: Option<Sender<Pedido>>,
    ultimo: Option<(String, Option<Atividade>)>,
    conectado: Arc<AtomicBool>,
}

impl Default for Presenca {
    fn default() -> Self {
        Self {
            envio: None,
            ultimo: None,
            conectado: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Presenca {
    /// Mostra `atividade`, ou desliga a presença com `None`.
    pub fn define(&mut self, atividade: Option<Atividade>) {
        let id = match atividade {
            Some(_) => APLICATIVO.to_string(),
            None => String::new(),
        };
        let pedido = (id.clone(), atividade);
        if self.ultimo.as_ref() == Some(&pedido) {
            return;
        }
        self.ultimo = Some(pedido.clone());
        if id.is_empty() {
            self.encerra();
            return;
        }
        let envio = self.envio.get_or_insert_with(|| {
            let (envio, recebe) = mpsc::channel();
            let conectado = Arc::clone(&self.conectado);
            let _ = std::thread::Builder::new()
                .name("discord".into())
                .spawn(move || laco(recebe, conectado));
            envio
        });
        if envio.send(Pedido::Mostra(pedido.0, pedido.1)).is_err() {
            self.envio = None;
        }
    }

    /// Se o Discord respondeu e a presença está sendo mostrada.
    pub fn conectado(&self) -> bool {
        self.conectado.load(Ordering::Relaxed)
    }

    fn encerra(&mut self) {
        if let Some(envio) = self.envio.take() {
            let _ = envio.send(Pedido::Encerra);
        }
        self.conectado.store(false, Ordering::Relaxed);
    }
}

impl Drop for Presenca {
    fn drop(&mut self) {
        self.encerra();
    }
}

/// A thread da conexão: guarda o último pedido e insiste até o Discord aceitá-lo.
fn laco(recebe: Receiver<Pedido>, conectado: Arc<AtomicBool>) {
    let mut cliente: Option<DiscordIpcClient> = None;
    let mut desejado: Option<(String, Option<Atividade>)> = None;
    let mut pendente = false;
    let mut proxima_tentativa = Instant::now();
    loop {
        let espera = match pendente {
            true => proxima_tentativa.saturating_duration_since(Instant::now()),
            false => Duration::from_secs(3600),
        };
        match recebe.recv_timeout(espera) {
            Ok(Pedido::Mostra(id, atividade)) => {
                // Trocar de aplicativo é outra conexão: o id vai no aperto de mão.
                if cliente.as_ref().is_some_and(|c| c.client_id != id) {
                    fecha(&mut cliente, &conectado);
                }
                desejado = Some((id, atividade));
                pendente = true;
                proxima_tentativa = Instant::now();
            }
            Ok(Pedido::Encerra) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(c) = cliente.as_mut() {
                    let _ = c.clear_activity();
                }
                fecha(&mut cliente, &conectado);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if !pendente || Instant::now() < proxima_tentativa {
            continue;
        }
        let Some((id, atividade)) = &desejado else {
            pendente = false;
            continue;
        };
        if cliente.is_none() {
            let mut novo = DiscordIpcClient::new(id);
            if novo.connect().is_ok() {
                cliente = Some(novo);
            }
        }
        let enviado = cliente.as_mut().is_some_and(|c| {
            match atividade {
                Some(atividade) => c.set_activity(monta(atividade)),
                None => c.clear_activity(),
            }
            .is_ok()
        });
        if enviado {
            conectado.store(true, Ordering::Relaxed);
            pendente = false;
        } else {
            // Conexão caída ou nunca aberta: começa do zero na próxima tentativa.
            fecha(&mut cliente, &conectado);
            proxima_tentativa = Instant::now() + NOVA_TENTATIVA;
        }
    }
}

fn fecha(cliente: &mut Option<DiscordIpcClient>, conectado: &AtomicBool) {
    if let Some(mut c) = cliente.take() {
        let _ = c.close();
    }
    conectado.store(false, Ordering::Relaxed);
}

fn monta(atividade: &Atividade) -> Activity<'_> {
    let mut assets = Assets::new()
        .large_image(atividade.imagem.as_str())
        .large_text(atividade.texto_da_imagem.as_str());
    if let Some((imagem, texto)) = &atividade.icone {
        assets = assets.small_image(imagem.as_str()).small_text(texto.as_str());
    }
    Activity::new()
        .activity_type(ActivityType::Playing)
        .details(atividade.detalhes.as_str())
        .assets(assets)
        .timestamps(Timestamps::new().start(atividade.inicio_ms))
}

/// A capa no quadrado que o Developer Portal aceita: a imagem inteira, centrada, sem cortar.
///
/// A ampliação é por vizinho mais próximo — a capa de um `.mif` tem poucas dezenas de pixels,
/// e borrar isso a 512 fica pior do que os pixels grandes.
pub fn capa_quadrada(imagem: &Image) -> Image {
    let lado = LADO_DA_CAPA;
    let mut rgba = vec![0u8; lado * lado * 4];
    if imagem.width == 0 || imagem.height == 0 {
        return Image { width: lado, height: lado, rgba };
    }
    let escala = lado as f64 / imagem.width.max(imagem.height) as f64;
    let largura = ((imagem.width as f64 * escala).round() as usize).clamp(1, lado);
    let altura = ((imagem.height as f64 * escala).round() as usize).clamp(1, lado);
    let (x0, y0) = ((lado - largura) / 2, (lado - altura) / 2);
    for y in 0..altura {
        let sy = (y * imagem.height / altura).min(imagem.height - 1);
        for x in 0..largura {
            let sx = (x * imagem.width / largura).min(imagem.width - 1);
            let de = (sy * imagem.width + sx) * 4;
            let para = ((y0 + y) * lado + x0 + x) * 4;
            rgba[para..para + 4].copy_from_slice(&imagem.rgba[de..de + 4]);
        }
    }
    Image { width: lado, height: lado, rgba }
}

/// A imagem em PNG, para gravar e cadastrar no aplicativo.
pub fn png(imagem: &Image) -> Result<Vec<u8>, String> {
    let mut saida = Vec::new();
    let mut encoder = png::Encoder::new(
        std::io::Cursor::new(&mut saida),
        imagem.width as u32,
        imagem.height as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut escritor = encoder.write_header().map_err(|e| e.to_string())?;
    escritor.write_image_data(&imagem.rgba).map_err(|e| e.to_string())?;
    escritor.finish().map_err(|e| e.to_string())?;
    Ok(saida)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mostra "No menu" no Discord aberto por dez segundos. Precisa do Discord rodando:
    /// `cargo test --release presenca_de_verdade -- --ignored`.
    #[test]
    #[ignore]
    fn presenca_de_verdade() {
        let mut cliente = DiscordIpcClient::new(APLICATIVO);
        cliente.connect().expect("o Discord está aberto?");
        let atividade = Atividade {
            detalhes: "No menu".into(),
            imagem: CHAVE_DO_ICONE.into(),
            texto_da_imagem: "Zeebx".into(),
            icone: None,
            inicio_ms: 0,
        };
        cliente.set_activity(monta(&atividade)).expect("set_activity");
        std::thread::sleep(Duration::from_secs(10));
        let _ = cliente.clear_activity();
        let _ = cliente.close();
    }

    #[test]
    fn a_chave_da_capa_so_tem_o_que_o_discord_aceita() {
        let chave = chave_da_capa(0x0108FF13);
        assert_eq!(chave, "jogo_0108ff13");
        assert!(chave.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
    }

    #[test]
    fn a_capa_vira_um_quadrado_de_512_centrado_sem_cortar() {
        // 2×1: vermelho à esquerda, verde à direita.
        let imagem = Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        let capa = capa_quadrada(&imagem);
        assert_eq!((capa.width, capa.height), (512, 512));
        let pixel = |x: usize, y: usize| &capa.rgba[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        // Faixa de 256 de altura no meio; acima dela, transparente.
        assert_eq!(pixel(10, 10), &[0, 0, 0, 0]);
        assert_eq!(pixel(10, 256), &[255, 0, 0, 255]);
        assert_eq!(pixel(500, 256), &[0, 255, 0, 255]);
    }
}
