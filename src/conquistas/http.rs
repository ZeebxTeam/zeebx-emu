//! O servidor de verdade: HTTP pelo `ureq`, uma thread por pedido. O rcheevos pede pouco, e cada
//! pedido espera a rede sem segurar o jogo.

use std::time::Duration;

use super::{Entrega, Pedido, Servidor};

pub struct ServidorHttp {
    agente: ureq::Agent,
    user_agent: String,
}

impl Default for ServidorHttp {
    fn default() -> Self {
        let agente = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            // O corpo de uma resposta de erro diz o motivo ("senha errada"), e o rcheevos o lê.
            .http_status_as_error(false)
            .build()
            .new_agent();
        let user_agent = String::new();
        Self { agente, user_agent }
    }
}

impl Servidor for ServidorHttp {
    fn define_user_agent(&mut self, user_agent: &str) {
        self.user_agent = user_agent.to_string();
    }

    fn pede(&mut self, pedido: Pedido, entrega: Entrega) {
        let agente = self.agente.clone();
        let user_agent = self.user_agent.clone();
        // Sem a thread, a entrega é descartada e responde sozinha como falha.
        let _ = std::thread::Builder::new()
            .name("conquistas-http".into())
            .spawn(move || {
                let resposta = match &pedido.corpo {
                    Some(corpo) => agente
                        .post(&pedido.url)
                        .header("User-Agent", &user_agent)
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .send(corpo.as_bytes()),
                    None => agente
                        .get(&pedido.url)
                        .header("User-Agent", &user_agent)
                        .call(),
                };
                match resposta
                    .and_then(|mut r| Ok((r.status().as_u16(), r.body_mut().read_to_vec()?)))
                {
                    Ok((status, corpo)) => entrega.responde(status, corpo),
                    Err(erro) => entrega.falha_de_rede(&erro.to_string()),
                }
            });
    }
}
