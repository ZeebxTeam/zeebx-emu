//! The real server: HTTP through `ureq`, one thread per request. rcheevos asks for little, and each
//! request waits for the network without holding up the game.

use std::time::Duration;

use super::{Delivery, Request, Server};

pub struct HttpServer {
    agent: ureq::Agent,
    user_agent: String,
}

impl Default for HttpServer {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            // The body of an error response says the reason ("wrong password"), and rcheevos reads it.
            .http_status_as_error(false)
            .build()
            .new_agent();
        let user_agent = String::new();
        Self { agent, user_agent }
    }
}

impl Server for HttpServer {
    fn set_user_agent(&mut self, user_agent: &str) {
        self.user_agent = user_agent.to_string();
    }

    fn request(&mut self, request: Request, delivery: Delivery) {
        let agent = self.agent.clone();
        let user_agent = self.user_agent.clone();
        // Without the thread, the delivery is dropped and responds on its own as a failure.
        let _ = std::thread::Builder::new()
            .name("achievements-http".into())
            .spawn(move || {
                let response = match &request.body {
                    Some(body) => agent
                        .post(&request.url)
                        .header("User-Agent", &user_agent)
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .send(body.as_bytes()),
                    None => agent
                        .get(&request.url)
                        .header("User-Agent", &user_agent)
                        .call(),
                };
                match response
                    .and_then(|mut r| Ok((r.status().as_u16(), r.body_mut().read_to_vec()?)))
                {
                    Ok((status, body)) => delivery.respond(status, body),
                    Err(error) => delivery.network_error(&error.to_string()),
                }
            });
    }
}
