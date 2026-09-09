//! O cliente HTTP do emulador.
//!
//! É pequeno de propósito. Os jogos do Zeebo falam **HTTP puro** — o Zeeboids aponta para
//! `http://www.zeeboids.com/...` e não há HTTPS em lugar nenhum do módulo —, então uma
//! biblioteca inteira traria TLS, redirecionamento, `keep-alive` e políticas que ninguém aqui
//! usa. O que se precisa é um POST com corpo binário e a resposta de volta.
//!
//! Dar rede a um binário de origem externa é decisão de projeto, e por isso ela não é
//! silenciosa: cada requisição entra no relatório, e o [`crate::machine::Machine`] pode ser
//! criado sem ela.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Quanto esperar por conexão e por resposta.
///
/// Cinco segundos é curto para um servidor de verdade e é justamente o que se quer: o emulador
/// não pode parar porque o outro lado não responde. O jogo já tem o próprio tempo limite, e
/// falhar rápido devolve a ele o controle.
const ESPERA: Duration = Duration::from_secs(5);

/// Teto de resposta que aceitamos guardar.
const MAX_RESPOSTA: usize = 1 << 20;

/// O que voltou do servidor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resposta {
    pub status: u16,
    pub corpo: Vec<u8>,
}

/// Quebra `http://maquina[:porta]/caminho` em (máquina, porta, caminho).
///
/// Só `http`. Um `https://` aqui seria um pedido que não sabemos atender, e responder como se
/// soubéssemos seria pior do que recusar.
pub fn separa(url: &str) -> Result<(String, u16, String), String> {
    let resto = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("não é uma URL http: {url:?}"))?;
    let (autoridade, caminho) = match resto.find('/') {
        Some(i) => (&resto[..i], &resto[i..]),
        None => (resto, "/"),
    };
    let (maquina, porta) = match autoridade.rsplit_once(':') {
        Some((m, p)) => (
            m,
            p.parse::<u16>()
                .map_err(|_| format!("porta inválida em {url:?}"))?,
        ),
        None => (autoridade, 80),
    };
    if maquina.is_empty() {
        return Err(format!("sem máquina em {url:?}"));
    }
    Ok((maquina.to_string(), porta, caminho.to_string()))
}

/// Manda um POST e devolve o que voltou.
///
/// O `Content-Type` é `application/octet-stream` porque é o que o jogo declara — as três strings
/// que descrevem a requisição no módulo do Zeeboids são `POST`, `X-Method: POST` e esse tipo.
pub fn post(url: &str, corpo: &[u8]) -> Result<Resposta, String> {
    let (maquina, porta, caminho) = separa(url)?;
    let alvo = format!("{maquina}:{porta}");
    let endereco = std::net::ToSocketAddrs::to_socket_addrs(&alvo)
        .map_err(|e| format!("não resolvi {alvo}: {e}"))?
        .next()
        .ok_or_else(|| format!("{alvo} não resolveu para endereço nenhum"))?;

    let mut fluxo =
        TcpStream::connect_timeout(&endereco, ESPERA).map_err(|e| format!("{alvo}: {e}"))?;
    fluxo.set_read_timeout(Some(ESPERA)).ok();
    fluxo.set_write_timeout(Some(ESPERA)).ok();

    let cabecalho = format!(
        "POST {caminho} HTTP/1.1\r\nHost: {maquina}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        corpo.len()
    );
    fluxo
        .write_all(cabecalho.as_bytes())
        .and_then(|()| fluxo.write_all(corpo))
        .and_then(|()| fluxo.flush())
        .map_err(|e| format!("ao enviar para {alvo}: {e}"))?;

    let mut bruto = Vec::new();
    fluxo
        .take(MAX_RESPOSTA as u64)
        .read_to_end(&mut bruto)
        .map_err(|e| format!("ao ler de {alvo}: {e}"))?;
    separa_resposta(&bruto)
}

/// Separa a resposta HTTP em status e corpo.
///
/// Não interpretamos mais que isso: `Transfer-Encoding: chunked` e afins ficariam para quando
/// aparecerem. Com `Connection: close`, o servidor simples que estamos do outro lado responde
/// com `Content-Length` e fecha.
pub fn separa_resposta(bruto: &[u8]) -> Result<Resposta, String> {
    let fim = bruto
        .windows(4)
        .position(|j| j == b"\r\n\r\n")
        .ok_or_else(|| "resposta sem fim de cabeçalho".to_string())?;
    let cabecalho = String::from_utf8_lossy(&bruto[..fim]);
    let status = cabecalho
        .lines()
        .next()
        .and_then(|linha| linha.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| {
            format!(
                "status ilegível em {:?}",
                &cabecalho[..cabecalho.len().min(40)]
            )
        })?;
    Ok(Resposta {
        status,
        corpo: bruto[fim + 4..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separa_url_com_e_sem_porta() {
        assert_eq!(
            separa("http://www.zeeboids.com/zeeboidsService/PHP/Client/synchronize.php").unwrap(),
            (
                "www.zeeboids.com".to_string(),
                80,
                "/zeeboidsService/PHP/Client/synchronize.php".to_string()
            )
        );
        assert_eq!(
            separa("http://127.0.0.1:8080/x").unwrap(),
            ("127.0.0.1".to_string(), 8080, "/x".to_string())
        );
        // Sem caminho, a raiz.
        assert_eq!(separa("http://exemplo").unwrap().2, "/");
    }

    #[test]
    fn recusa_o_que_nao_sabemos_atender() {
        assert!(separa("https://exemplo/x").is_err());
        assert!(separa("socket://zeebo-cust.opera-mini.net:1080/").is_err());
        assert!(separa("http://:80/x").is_err());
        assert!(separa("http://exemplo:porta/x").is_err());
    }

    #[test]
    fn le_status_e_corpo() {
        let bruto = b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n1;100001;";
        let r = separa_resposta(bruto).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.corpo, b"1;100001;");

        // Corpo vazio é resposta legítima: o `synchronize.php` responde assim.
        let r = separa_resposta(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
        assert_eq!((r.status, r.corpo.len()), (200, 0));
        assert!(separa_resposta(b"lixo").is_err());
    }
}
