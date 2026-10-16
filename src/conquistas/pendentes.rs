//! Os desbloqueios e os placares que ainda não chegaram ao servidor, guardados em disco.
//!
//! O rcheevos guarda os pedidos que falharam **só na memória**: ele tenta de novo enquanto o
//! emulador está aberto, e fechar o Zeebx sem rede perderia o desbloqueio. O RA exige que eles
//! fiquem guardados e sejam enviados quando a rede voltar. Por isso cada pedido de desbloqueio e de
//! placar é anotado **antes** de sair, e só sai do arquivo com uma resposta definitiva do servidor.
//! O que sobrar de uma execução é mandado de novo na seguinte, depois do login.
//!
//! **Guarda-se o que o pedido significa, e não o pedido.** A primeira versão guardava o corpo e a
//! URL, e as duas coisas falharam na janela de verdade: a cada nova tentativa o rcheevos acrescenta
//! `o=<segundos desde o desbloqueio>`, que muda a assinatura `v`, e doze tentativas de duas
//! conquistas viraram dez entradas; e a URL guardava o host da execução anterior. Agora a entrada é
//! a conquista (ou o placar e o valor), o modo, o hash do jogo e o **instante** em que aconteceu, e
//! o pedido é remontado pelo próprio rcheevos na hora de mandar, com o host, o token e o `o` de
//! então (ver `Conquistas::reenvia_pendentes`).
//!
//! O token não vai para o arquivo: o pedido remontado usa o do login atual, e uma entrada de outra
//! conta espera o login dela.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Pedido;

/// Onde a fila fica: `<config>/conquistas/pendentes.json`.
pub fn caminho_padrao() -> PathBuf {
    crate::config::config_dir()
        .join("conquistas")
        .join("pendentes.json")
}

/// O que o pedido pedia.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum OQue {
    Desbloqueio { conquista: u32, hardcore: bool },
    Placar { placar: u32, valor: i32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pendente {
    #[serde(flatten)]
    pub o_que: OQue,
    pub usuario: String,
    /// O hash do jogo em que aconteceu.
    pub hash: String,
    /// Quando aconteceu, em segundos desde 1970. O servidor registra o desbloqueio nesse instante,
    /// e não no do envio.
    pub quando: u64,
}

impl Pendente {
    /// As tentativas repetidas do mesmo desbloqueio são uma entrada só.
    fn chave(&self) -> String {
        match self.o_que {
            OQue::Desbloqueio {
                conquista,
                hardcore,
            } => format!("a/{}/{conquista}/{}", self.usuario, u8::from(hardcore)),
            OQue::Placar { placar, valor } => {
                format!("p/{}/{placar}/{valor}/{}", self.usuario, self.hash)
            }
        }
    }
}

/// A fila em disco. Sem caminho, ela não guarda nada: é o caso dos testes que não a pedem.
#[derive(Default)]
pub struct Pendentes {
    caminho: Option<PathBuf>,
    itens: Vec<Pendente>,
    /// As chaves que vieram do disco na abertura, e que ainda não foram mandadas de novo.
    anteriores: Vec<String>,
}

impl Pendentes {
    /// Sem disco.
    pub fn desligada() -> Self {
        Self::default()
    }

    /// A fila de `caminho`, com o que uma execução anterior deixou.
    pub fn em(caminho: PathBuf) -> Self {
        let itens: Vec<Pendente> = std::fs::read(&caminho)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let anteriores = itens.iter().map(Pendente::chave).collect();
        Self {
            caminho: Some(caminho),
            itens,
            anteriores,
        }
    }

    /// Anota o pedido, se ele é dos que não podem se perder. `agora` é o relógio do mundo, em
    /// segundos desde 1970. Devolve a chave para o [`Pendentes::resolve`] da resposta.
    pub(super) fn anota(&mut self, pedido: &Pedido, agora: u64) -> Option<String> {
        self.caminho.as_ref()?;
        let pendente = interpreta(pedido.corpo.as_deref()?, agora)?;
        let chave = pendente.chave();
        if !self.itens.iter().any(|item| item.chave() == chave) {
            self.itens.push(pendente);
            self.grava();
        }
        Some(chave)
    }

    /// A resposta de um pedido anotado chegou. Uma resposta definitiva tira o pedido da fila;
    /// falha de rede, erro do servidor (5xx) e "devagar" (429) o deixam lá.
    pub(super) fn resolve(&mut self, chave: &str, status: i32) {
        if !definitiva(status) {
            return;
        }
        let antes = self.itens.len();
        self.itens.retain(|item| item.chave() != chave);
        if self.itens.len() != antes {
            self.grava();
        }
    }

    /// Os pedidos de `usuario` que uma execução anterior deixou, para mandar de novo. Cada um sai
    /// só uma vez por execução; os de outra conta esperam o login dela.
    pub(super) fn da_execucao_anterior(&mut self, usuario: &str) -> Vec<(String, Pendente)> {
        let mut deste = Vec::new();
        self.anteriores.retain(|chave| {
            match self
                .itens
                .iter()
                .find(|item| item.chave() == *chave && item.usuario == usuario)
            {
                Some(item) => {
                    deste.push((chave.clone(), item.clone()));
                    false
                }
                None => true,
            }
        });
        deste
    }

    /// Quantos pedidos esperam o servidor.
    pub fn quantos(&self) -> usize {
        self.itens.len()
    }

    /// Grava a fila. Uma falha de disco fica no registro: o rcheevos continua tentando na
    /// memória, e o pior caso é o de antes desta fila existir.
    fn grava(&self) {
        let Some(caminho) = &self.caminho else {
            return;
        };
        let resultado = serde_json::to_vec_pretty(&self.itens)
            .map_err(|erro| erro.to_string())
            .and_then(|bytes| escreve(caminho, &bytes));
        if let Err(erro) = resultado {
            crate::registro!(
                crate::registro::Nivel::Aviso,
                "conquistas",
                "a fila de desbloqueios pendentes não foi gravada em {}: {erro}",
                caminho.display()
            );
        }
    }
}

/// O que um corpo de pedido do rcheevos pede, se é um dos que não podem se perder: o desbloqueio
/// e o envio de placar. O rich presence e o login se refazem sozinhos.
fn interpreta(corpo: &str, agora: u64) -> Option<Pendente> {
    let parametros: Vec<(String, String)> = corpo
        .split('&')
        .filter_map(|par| {
            let (chave, valor) = par.split_once('=')?;
            Some((chave.to_string(), decodifica(valor)))
        })
        .collect();
    let valor = |nome: &str| {
        parametros
            .iter()
            .find(|(chave, _)| chave == nome)
            .map(|(_, valor)| valor.as_str())
    };
    let o_que = match valor("r")? {
        "awardachievement" => OQue::Desbloqueio {
            conquista: valor("a")?.parse().ok()?,
            hardcore: valor("h") == Some("1"),
        },
        "submitlbentry" => OQue::Placar {
            placar: valor("i")?.parse().ok()?,
            valor: valor("s")?.parse().ok()?,
        },
        _ => return None,
    };
    // A nova tentativa diz há quantos segundos aconteceu: o instante é o mesmo da primeira.
    let atras: u64 = valor("o").and_then(|o| o.parse().ok()).unwrap_or(0);
    Some(Pendente {
        o_que,
        usuario: valor("u")?.to_string(),
        hash: valor("m").unwrap_or_default().to_string(),
        quando: agora.saturating_sub(atras),
    })
}

/// Desfaz o `%xx` e o `+` de um valor de formulário. Um `%` sem dois dígitos hexadecimais depois
/// fica como está.
fn decodifica(valor: &str) -> String {
    let bytes = valor.as_bytes();
    let hexa = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut saida = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let par = bytes
            .get(i + 1)
            .zip(bytes.get(i + 2))
            .and_then(|(&alto, &baixo)| Some(hexa(alto)? << 4 | hexa(baixo)?));
        match (bytes[i], par) {
            (b'+', _) => saida.push(b' '),
            (b'%', Some(b)) => {
                saida.push(b);
                i += 2;
            }
            (b, _) => saida.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&saida).into_owned()
}

/// Uma resposta que o servidor não vai mudar se o pedido for repetido. Um 4xx é recusa de verdade
/// (conquista inexistente, token vencido), e repetir não adianta; o 429 é "devagar", e vale tentar.
fn definitiva(status: i32) -> bool {
    (200..500).contains(&status) && status != 429
}

/// Grava num arquivo temporário ao lado e troca, para um corte no meio não deixar o JSON pela
/// metade.
fn escreve(caminho: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;

    if let Some(pasta) = caminho.parent() {
        std::fs::create_dir_all(pasta).map_err(|erro| erro.to_string())?;
    }
    let temporario = caminho.with_extension("parcial");
    let mut arquivo = std::fs::File::create(&temporario).map_err(|erro| erro.to_string())?;
    arquivo.write_all(bytes).map_err(|erro| erro.to_string())?;
    arquivo.sync_all().map_err(|erro| erro.to_string())?;
    std::fs::rename(&temporario, caminho).map_err(|erro| erro.to_string())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn pedido(corpo: &str) -> Pedido {
        Pedido {
            url: "https://retroachievements.org/dorequest.php".into(),
            corpo: Some(corpo.into()),
            tipo: Some("application/x-www-form-urlencoded".into()),
        }
    }

    const DESBLOQUEIO: &str = "r=awardachievement&u=User&t=T&a=5&h=0&m=abc&v=1";

    #[test]
    fn so_o_desbloqueio_e_o_placar_entram_na_fila() {
        let pasta = crate::scratch::TempDir::new("zeebx-pendentes-quais");
        let mut fila = Pendentes::em(pasta.join("pendentes.json"));
        assert!(fila.anota(&pedido("r=login2&u=User&p=x"), 100).is_none());
        assert!(fila.anota(&pedido("r=ping&u=User&t=T&g=1"), 100).is_none());
        assert!(fila.anota(&pedido(DESBLOQUEIO), 100).is_some());
        assert!(
            fila.anota(&pedido("r=submitlbentry&u=User&t=T&i=4&s=-17&m=abc"), 100)
                .is_some()
        );
        assert_eq!(fila.quantos(), 2);
    }

    /// O que o rcheevos manda na janela de verdade: a segunda tentativa traz `o` e outra `v`.
    #[test]
    fn as_tentativas_com_o_e_v_diferentes_sao_uma_entrada_so_com_o_instante_da_primeira() {
        let pasta = crate::scratch::TempDir::new("zeebx-pendentes-repetida");
        let mut fila = Pendentes::em(pasta.join("pendentes.json"));
        let a = fila.anota(&pedido(DESBLOQUEIO), 1000);
        let b = fila.anota(
            &pedido("r=awardachievement&u=User&t=T&a=5&h=0&m=abc&o=3&v=2"),
            1003,
        );
        assert_eq!(a, b);
        assert_eq!(fila.quantos(), 1);
        assert_eq!(fila.itens[0].quando, 1000);
    }

    #[test]
    fn o_token_nao_vai_para_o_arquivo() {
        let pasta = crate::scratch::TempDir::new("zeebx-pendentes-token");
        let caminho = pasta.join("pendentes.json");
        Pendentes::em(caminho.clone()).anota(
            &pedido("r=awardachievement&u=User&t=segredo&a=5&h=1&m=abc"),
            1,
        );
        let texto = std::fs::read_to_string(&caminho).unwrap();
        assert!(!texto.contains("segredo"), "{texto}");
        assert!(texto.contains("\"conquista\": 5"), "{texto}");
    }

    #[test]
    fn so_a_resposta_definitiva_tira_da_fila() {
        let pasta = crate::scratch::TempDir::new("zeebx-pendentes-resposta");
        let mut fila = Pendentes::em(pasta.join("pendentes.json"));
        let chave = fila.anota(&pedido(DESBLOQUEIO), 1).unwrap();
        for status in [-2, -1, 500, 503, 429] {
            fila.resolve(&chave, status);
            assert_eq!(fila.quantos(), 1, "{status} tirou da fila");
        }
        fila.resolve(&chave, 200);
        assert_eq!(fila.quantos(), 0);
    }

    /// O que uma execução deixou volta na seguinte, uma vez, só para a conta dele; o que nasceu
    /// nesta não é mandado de novo por ela.
    #[test]
    fn a_fila_sobrevive_ao_fechamento_e_volta_uma_vez_para_a_conta_certa() {
        let pasta = crate::scratch::TempDir::new("zeebx-pendentes-abertura");
        let caminho = pasta.join("pendentes.json");
        let mut primeira = Pendentes::em(caminho.clone());
        primeira.anota(&pedido(DESBLOQUEIO), 1);
        primeira.anota(&pedido("r=awardachievement&u=Outra&t=T&a=7&h=0&m=abc"), 1);

        let mut seguinte = Pendentes::em(caminho.clone());
        seguinte.anota(&pedido("r=awardachievement&u=User&t=T&a=6&h=1&m=abc"), 2);
        let anteriores = seguinte.da_execucao_anterior("User");
        assert_eq!(anteriores.len(), 1);
        assert_eq!(
            anteriores[0].1.o_que,
            OQue::Desbloqueio {
                conquista: 5,
                hardcore: false
            }
        );
        assert!(
            seguinte.da_execucao_anterior("User").is_empty(),
            "duas vezes"
        );
        assert_eq!(seguinte.da_execucao_anterior("Outra").len(), 1);
        assert_eq!(seguinte.quantos(), 3);
    }

    #[test]
    fn o_valor_do_formulario_e_decodificado() {
        assert_eq!(decodifica("Pa%24%24word"), "Pa$$word");
        assert_eq!(decodifica("um+dois"), "um dois");
        assert_eq!(decodifica("100%"), "100%");
        assert_eq!(decodifica("%zz"), "%zz");
        assert_eq!(decodifica("%é"), "%é");
    }
}
