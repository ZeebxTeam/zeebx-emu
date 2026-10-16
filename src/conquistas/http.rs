//! O servidor de verdade: HTTP pelo `ureq`, uma thread por pedido.
//!
//! **Uma thread por pedido, e não um laço.** O rcheevos pede pouco — o login, o jogo, a sessão,
//! um desbloqueio de vez em quando, o rich presence a cada dois minutos —, e cada pedido espera a
//! rede sem segurar o jogo. A resposta volta pela [`Entrega`], e o [`super::Conquistas`] a passa
//! ao rcheevos na thread dele, no quadro seguinte.

use std::time::Duration;

use super::{Entrega, Pedido, Servidor};

/// Quanto um pedido espera. O desbloqueio que não chega é tentado de novo pelo rcheevos.
const PRAZO: Duration = Duration::from_secs(30);

pub struct ServidorHttp {
    agente: ureq::Agent,
    user_agent: String,
}

impl ServidorHttp {
    pub fn novo() -> Self {
        let agente = ureq::Agent::config_builder()
            .timeout_global(Some(PRAZO))
            // O corpo de uma resposta de erro é o que diz o motivo ("senha errada"): o rcheevos
            // precisa dele, e não de um erro do ureq no lugar.
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self {
            agente,
            user_agent: format!("Zeebx/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl Default for ServidorHttp {
    fn default() -> Self {
        Self::novo()
    }
}

impl Servidor for ServidorHttp {
    fn define_user_agent(&mut self, user_agent: &str) {
        self.user_agent = user_agent.to_string();
    }

    fn pede(&mut self, pedido: Pedido, entrega: Entrega) {
        let agente = self.agente.clone();
        let user_agent = self.user_agent.clone();
        // Se a thread não nascer, o fechamento é descartado com a entrega dentro, e ela responde
        // sozinha como falha: ver o `Drop` da [`Entrega`].
        let _ = std::thread::Builder::new()
            .name("conquistas-http".into())
            .spawn(move || match faz(&agente, &user_agent, &pedido) {
                Ok((status, corpo)) => entrega.responde(status, corpo),
                Err(erro) => entrega.falha_de_rede(&erro),
            });
    }
}

/// O pedido, do jeito que o rcheevos o montou: POST com o corpo, ou GET sem ele.
fn faz(agente: &ureq::Agent, user_agent: &str, pedido: &Pedido) -> Result<(u16, Vec<u8>), String> {
    let resposta = match &pedido.corpo {
        Some(corpo) => agente
            .post(&pedido.url)
            .header("User-Agent", user_agent)
            .header(
                "Content-Type",
                pedido
                    .tipo
                    .as_deref()
                    .unwrap_or("application/x-www-form-urlencoded"),
            )
            .send(corpo.as_bytes()),
        None => agente
            .get(&pedido.url)
            .header("User-Agent", user_agent)
            .call(),
    };
    let mut resposta = resposta.map_err(|erro| erro.to_string())?;
    let status = resposta.status().as_u16();
    let corpo = resposta
        .body_mut()
        .read_to_vec()
        .map_err(|erro| erro.to_string())?;
    Ok((status, corpo))
}

/// Baixa `url` para `destino`, com um arquivo temporário ao lado: um download interrompido não
/// deixa um PNG pela metade no lugar do ícone.
pub fn baixa(url: &str, destino: &std::path::Path) -> Result<(), String> {
    let agente = ureq::Agent::config_builder()
        .timeout_global(Some(PRAZO))
        .build()
        .new_agent();
    let mut resposta = agente
        .get(url)
        .header(
            "User-Agent",
            &format!("Zeebx/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|erro| erro.to_string())?;
    let bytes = resposta
        .body_mut()
        .read_to_vec()
        .map_err(|erro| erro.to_string())?;
    if let Some(pasta) = destino.parent() {
        std::fs::create_dir_all(pasta).map_err(|erro| erro.to_string())?;
    }
    let parcial = destino.with_extension("parcial");
    std::fs::write(&parcial, bytes).map_err(|erro| erro.to_string())?;
    std::fs::rename(&parcial, destino).map_err(|erro| erro.to_string())
}

#[cfg(test)]
mod testes {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    use super::ServidorHttp;
    use crate::conquistas::{Conquistas, Evento};

    /// Um servidor HTTP de uma resposta só, em `127.0.0.1`: devolve o que leu do pedido.
    fn servidor_de_uma_resposta(corpo: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let ouvinte = TcpListener::bind("127.0.0.1:0").unwrap();
        let endereco = format!("http://{}", ouvinte.local_addr().unwrap());
        let fio = std::thread::spawn(move || {
            let (conexao, _) = ouvinte.accept().unwrap();
            let mut leitor = BufReader::new(conexao.try_clone().unwrap());
            let mut pedido = String::new();
            let mut tamanho = 0;
            loop {
                let mut linha = String::new();
                leitor.read_line(&mut linha).unwrap();
                if let Some(valor) = linha.to_ascii_lowercase().strip_prefix("content-length:") {
                    tamanho = valor.trim().parse().unwrap();
                }
                pedido.push_str(&linha);
                if linha == "\r\n" {
                    break;
                }
            }
            let mut dados = vec![0u8; tamanho];
            leitor.read_exact(&mut dados).unwrap();
            pedido.push_str(&String::from_utf8_lossy(&dados));
            let mut conexao = conexao;
            write!(
                conexao,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{corpo}",
                corpo.len()
            )
            .unwrap();
            pedido
        });
        (endereco, fio)
    }

    /// O pedido sai como o RA espera — POST, corpo de formulário, o user agent do Zeebx com o do
    /// rcheevos —, e a resposta, que chega por outra thread, volta ao cliente no `ocioso`.
    #[test]
    fn o_login_vai_por_http_e_a_resposta_volta_pela_fila() {
        let (endereco, fio) = servidor_de_uma_resposta(
            "{\"Success\":true,\"User\":\"User\",\"Token\":\"ApiToken\",\"Score\":1,\"SoftcoreScore\":2}",
        );
        let mut c = Conquistas::novo(Box::new(ServidorHttp::novo()));
        c.define_host(Some(&endereco));
        c.entra_com_senha("User", "Pa$$word");
        let inicio = Instant::now();
        let mut eventos = Vec::new();
        while eventos.is_empty() && inicio.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
            c.ocioso(None);
            eventos = c.eventos();
        }
        assert_eq!(eventos, vec![Evento::Entrou(Ok(()))]);
        assert_eq!(c.usuario().map(|u| u.token).as_deref(), Some("ApiToken"));

        let pedido = fio.join().unwrap();
        assert!(pedido.starts_with("POST /dorequest.php"), "{pedido}");
        let minusculo = pedido.to_ascii_lowercase();
        assert!(
            minusculo.contains("content-type: application/x-www-form-urlencoded"),
            "{pedido}"
        );
        let ua = format!("user-agent: {}", c.user_agent()).to_ascii_lowercase();
        assert!(minusculo.contains(&ua), "{pedido}");
        assert!(
            pedido.contains("r=login2") && pedido.contains("u=User"),
            "{pedido}"
        );
    }
}
