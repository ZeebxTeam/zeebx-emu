//! O **formato** do save state: versionado, com seções nomeadas e conferência de integridade.
//!
//! Existe separado do estado de propósito. O `retro_serialize_size` do core responde **zero**
//! enquanto o estado não estiver inteiro dentro dele — zero é como o frontend entende "este core
//! não salva", e é a única resposta honesta enquanto houver campo de fora. O que este módulo
//! resolve primeiro é a parte que o objetivo nomeia: **formato versionado**.
//!
//! ## Por que seções nomeadas
//!
//! O estado de uma máquina tem cento e noventa campos, e boa parte deles são tabelas de objetos do
//! guest. Um bloco opaco de bytes obrigaria a salvar e a carregar na mesma ordem para sempre: mudar
//! o campo de lugar viraria save state incompatível **em silêncio**. Com nome, cada pedaço se
//! acha, e uma seção que o leitor não conhece é **pulada** — é o que permite ler um estado gravado
//! por uma versão mais nova sem adivinhar.
//!
//! ## Integridade
//!
//! O `crc32` do conteúdo vai no cabeçalho. Sem ele, um arquivo truncado por falta de espaço em
//! disco carregaria como estado válido, e o jogo quebraria em algum lugar sem relação com o
//! problema. Com ele, a recusa acontece na porta.
//!
//! ## Layout
//!
//! ```text
//! 0..4    assinatura "ZBXS"
//! 4..6    versão do formato, u16 (little-endian)
//! 6..10   tamanho do conteúdo, u32
//! 10..14  crc32 do conteúdo, u32
//! 14..    conteúdo: seções, uma após a outra
//! ```
//!
//! Cada seção é `u8` com o tamanho do nome, o nome, `u32` com o tamanho dos dados, e os dados.

use std::collections::BTreeMap;
use std::ops::Range;

/// Assinatura do arquivo. Quatro bytes que dizem "isto é um save state do Zeebx".
pub const ASSINATURA: [u8; 4] = *b"ZBXS";

/// Versão do formato. Sobe quando o **significado** das seções muda, não quando uma seção nova
/// aparece: seção desconhecida é pulada pelo leitor, e é assim que um estado antigo continua
/// legível depois de o motor ganhar um campo.
pub const VERSAO: u16 = 1;

/// O tamanho do cabeçalho, em bytes.
const CABECALHO: usize = 14;

/// O que deu errado ao abrir um estado.
///
/// Cada variante diz **o que** estava errado e **com o que** se deparou. "Save state inválido"
/// sozinho obriga quem lê a investigar o arquivo a partir do zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Erro {
    /// O arquivo não começa com [`ASSINATURA`].
    Assinatura([u8; 4]),
    /// O arquivo tem uma versão que este motor não sabe ler.
    Versao { encontrada: u16, suportada: u16 },
    /// O arquivo acaba antes do que o cabeçalho promete.
    Truncado { esperado: usize, encontrado: usize },
    /// O conteúdo não bate com o `crc32` do cabeçalho.
    Integridade { esperado: u32, encontrado: u32 },
    /// Uma seção está malformada por dentro.
    Secao { nome: String, motivo: String },
}

impl std::fmt::Display for Erro {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Erro::Assinatura(bytes) => write!(
                f,
                "não é um save state do Zeebx: os quatro primeiros bytes são {bytes:02x?}, \
                 e deviam ser {ASSINATURA:02x?}"
            ),
            Erro::Versao {
                encontrada,
                suportada,
            } => write!(
                f,
                "save state da versão {encontrada}, e este motor lê até a {suportada}"
            ),
            Erro::Truncado {
                esperado,
                encontrado,
            } => write!(
                f,
                "save state cortado: o cabeçalho promete {esperado} bytes e o arquivo tem {encontrado}"
            ),
            Erro::Integridade {
                esperado,
                encontrado,
            } => write!(
                f,
                "save state corrompido: crc32 {encontrado:08x} onde o cabeçalho diz {esperado:08x}"
            ),
            Erro::Secao { nome, motivo } => {
                write!(f, "seção \"{nome}\" malformada: {motivo}")
            }
        }
    }
}

impl std::error::Error for Erro {}

/// Grava as seções no formato. A ordem em que entram é a ordem em que saem no arquivo.
pub fn escreve(secoes: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut conteudo = Vec::new();
    for (nome, dados) in secoes {
        let nome = nome.as_bytes();
        // O nome cabe num byte: são nomes de seção, não caminhos.
        let tamanho_do_nome = u8::try_from(nome.len().min(u8::MAX as usize)).unwrap_or(u8::MAX);
        conteudo.push(tamanho_do_nome);
        conteudo.extend_from_slice(&nome[..tamanho_do_nome as usize]);
        conteudo.extend_from_slice(&(dados.len() as u32).to_le_bytes());
        conteudo.extend_from_slice(dados);
    }

    let mut saida = Vec::with_capacity(CABECALHO + conteudo.len());
    saida.extend_from_slice(&ASSINATURA);
    saida.extend_from_slice(&VERSAO.to_le_bytes());
    saida.extend_from_slice(&(conteudo.len() as u32).to_le_bytes());
    saida.extend_from_slice(&crc32(&conteudo).to_le_bytes());
    saida.extend_from_slice(&conteudo);
    saida
}

/// O `crc32` que o cabeçalho guarda.
fn crc32(dados: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(dados);
    crc.sum()
}

/// Um estado aberto, pronto para ter as seções lidas.
#[derive(Debug)]
pub struct Leitor<'a> {
    dados: &'a [u8],
    secoes: BTreeMap<String, Range<usize>>,
}

impl<'a> Leitor<'a> {
    /// Confere o cabeçalho e indexa as seções.
    ///
    /// **Não** interpreta nenhuma seção: quem sabe o que cada uma significa é o dono do estado.
    pub fn abre(arquivo: &'a [u8]) -> Result<Self, Erro> {
        if arquivo.len() < CABECALHO {
            return Err(Erro::Truncado {
                esperado: CABECALHO,
                encontrado: arquivo.len(),
            });
        }
        let assinatura = [arquivo[0], arquivo[1], arquivo[2], arquivo[3]];
        if assinatura != ASSINATURA {
            return Err(Erro::Assinatura(assinatura));
        }
        let versao = u16::from_le_bytes([arquivo[4], arquivo[5]]);
        if versao > VERSAO {
            return Err(Erro::Versao {
                encontrada: versao,
                suportada: VERSAO,
            });
        }
        let prometido = u32::from_le_bytes([arquivo[6], arquivo[7], arquivo[8], arquivo[9]]) as usize;
        let esperado = u32::from_le_bytes([arquivo[10], arquivo[11], arquivo[12], arquivo[13]]);
        let disponivel = arquivo.len() - CABECALHO;
        if disponivel < prometido {
            return Err(Erro::Truncado {
                esperado: CABECALHO + prometido,
                encontrado: arquivo.len(),
            });
        }
        let conteudo = &arquivo[CABECALHO..CABECALHO + prometido];
        let encontrado = crc32(conteudo);
        if encontrado != esperado {
            return Err(Erro::Integridade {
                esperado,
                encontrado,
            });
        }

        let mut secoes = BTreeMap::new();
        let mut posicao = 0usize;
        while posicao < conteudo.len() {
            let tamanho_do_nome = conteudo[posicao] as usize;
            posicao += 1;
            if posicao + tamanho_do_nome > conteudo.len() {
                return Err(Erro::Secao {
                    nome: "?".to_string(),
                    motivo: "o cabeçalho da seção passa do fim do arquivo".to_string(),
                });
            }
            let nome = String::from_utf8_lossy(&conteudo[posicao..posicao + tamanho_do_nome])
                .into_owned();
            posicao += tamanho_do_nome;
            if posicao + 4 > conteudo.len() {
                return Err(Erro::Secao {
                    nome,
                    motivo: "falta o tamanho dos dados".to_string(),
                });
            }
            let tamanho = u32::from_le_bytes([
                conteudo[posicao],
                conteudo[posicao + 1],
                conteudo[posicao + 2],
                conteudo[posicao + 3],
            ]) as usize;
            posicao += 4;
            if posicao + tamanho > conteudo.len() {
                return Err(Erro::Secao {
                    nome,
                    motivo: format!(
                        "diz ter {tamanho} bytes e restam {}",
                        conteudo.len() - posicao
                    ),
                });
            }
            secoes.insert(nome, posicao..posicao + tamanho);
            posicao += tamanho;
        }
        Ok(Self {
            dados: conteudo,
            secoes,
        })
    }

    /// Os dados de uma seção, ou `None` se o arquivo não a tem.
    pub fn secao(&self, nome: &str) -> Option<&'a [u8]> {
        self.secoes.get(nome).map(|faixa| &self.dados[faixa.clone()])
    }

    /// Os nomes das seções, em ordem.
    pub fn nomes(&self) -> Vec<&str> {
        self.secoes.keys().map(String::as_str).collect()
    }
}

/// Um pedaço do estado que sabe se gravar e se restaurar.
///
/// Cada subsistema do motor implementa isto no próprio arquivo, onde os campos são conhecidos. O
/// que fica aqui é só o contrato: gravar devolve bytes por nome, restaurar os consome e **recusa**
/// o que não bate — nunca aplica metade.
pub trait Guardavel {
    /// Escreve o estado nas seções.
    fn grava(&self, destino: &mut Secoes);

    /// Lê o estado das seções.
    fn restaura(&mut self, origem: &Leitor<'_>) -> Result<(), Erro>;
}

/// Monta o conteúdo de um estado, seção por seção.
#[derive(Debug, Default)]
pub struct Secoes {
    pares: Vec<(String, Vec<u8>)>,
}

impl Secoes {
    pub fn nova() -> Self {
        Self::default()
    }

    /// Guarda bytes crus numa seção.
    pub fn poe(&mut self, nome: &str, bytes: Vec<u8>) {
        self.pares.push((nome.to_string(), bytes));
    }

    /// Guarda um número.
    pub fn poe_u32(&mut self, nome: &str, valor: u32) {
        self.poe(nome, valor.to_le_bytes().to_vec());
    }

    /// Guarda uma lista de números, com a contagem na frente.
    /// Grava um mapa, **em ordem de endereço**, para o arquivo não depender da ordem do `HashMap`.
    pub fn poe_mapa(&mut self, nome: &str, mapa: impl IntoIterator<Item = (u32, u32)>) {
        let mut pares: Vec<(u32, u32)> = mapa.into_iter().collect();
        pares.sort_unstable();
        self.poe_u32s(nome, pares.into_iter().flat_map(|(a, b)| [a, b]));
    }

    pub fn poe_u32s(&mut self, nome: &str, valores: impl IntoIterator<Item = u32>) {
        let valores: Vec<u32> = valores.into_iter().collect();
        let mut bytes = Vec::with_capacity(4 + valores.len() * 4);
        bytes.extend_from_slice(&(valores.len() as u32).to_le_bytes());
        for valor in valores {
            bytes.extend_from_slice(&valor.to_le_bytes());
        }
        self.poe(nome, bytes);
    }

    /// Grava uma lista de textos: a contagem e, depois, cada texto com o tamanho na frente.
    pub fn poe_textos(&mut self, nome: &str, textos: impl IntoIterator<Item = String>) {
        let textos: Vec<String> = textos.into_iter().collect();
        let mut saida = Vec::new();
        saida.extend_from_slice(&(textos.len() as u32).to_le_bytes());
        for texto in textos {
            let bytes = texto.as_bytes();
            saida.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            saida.extend_from_slice(bytes);
        }
        self.poe(nome, saida);
    }

    /// Grava blocos de bytes indexados por uma chave de `chaves` números cada.
    ///
    /// É a forma de toda tabela que guarda **conteúdo** em vez de número: as preferências, os
    /// parâmetros de coleção, os dados que o `IConfig` recebeu. `chaves` diz quantos números formam
    /// a chave — um para `HashMap<u32, Vec<u8>>`, dois para `HashMap<(u32, u32), Vec<u8>>`.
    pub fn poe_blocos(
        &mut self,
        nome: &str,
        chaves: usize,
        itens: impl IntoIterator<Item = (Vec<u32>, Vec<u8>)>,
    ) {
        let itens: Vec<(Vec<u32>, Vec<u8>)> = itens.into_iter().collect();
        let mut saida = Vec::new();
        saida.extend_from_slice(&(itens.len() as u32).to_le_bytes());
        saida.extend_from_slice(&(chaves as u32).to_le_bytes());
        for (chave, dados) in itens {
            debug_assert_eq!(chave.len(), chaves);
            for n in chave {
                saida.extend_from_slice(&n.to_le_bytes());
            }
            saida.extend_from_slice(&(dados.len() as u32).to_le_bytes());
            saida.extend_from_slice(&dados);
        }
        self.poe(nome, saida);
    }

    /// Grava um texto em UTF-8, com o tamanho na frente.
    ///
    /// Caminho de arquivo, nome de fonte, título: o que não é número vai assim. O tamanho vem
    /// antes porque um separador teria de ser um byte que o texto não pode conter — e num caminho
    /// de arquivo isso não existe.
    pub fn poe_texto(&mut self, nome: &str, texto: &str) {
        let bytes = texto.as_bytes();
        let mut saida = Vec::with_capacity(4 + bytes.len());
        saida.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        saida.extend_from_slice(bytes);
        self.poe(nome, saida);
    }

    /// Grava **registros de tamanho fixo**: cada um é o índice do objeto seguido dos campos dele.
    ///
    /// É a forma da maioria das tabelas do motor — `HashMap<u32, Estado>` onde o estado é um punhado
    /// de números. Com um ajudante só, cada tabela nova vira cinco linhas de gravação e cinco de
    /// leitura, e não um formato próprio que alguém precisa entender de novo.
    pub fn poe_registros(
        &mut self,
        nome: &str,
        registros: impl IntoIterator<Item = Vec<u32>>,
    ) {
        let registros: Vec<u32> = registros.into_iter().flatten().collect();
        self.poe_u32s(nome, registros);
    }

    /// Grava trios de números, com a contagem na frente.
    pub fn poe_trios(&mut self, nome: &str, valores: impl IntoIterator<Item = (u32, u32, u32)>) {
        let valores: Vec<u32> = valores
            .into_iter()
            .flat_map(|(a, b, c)| [a, b, c])
            .collect();
        self.poe_u32s(nome, valores);
    }

    /// Troca o conteúdo de uma seção, ou acrescenta se ela não existir.
    ///
    /// Existe para os testes de recusa: a maneira honesta de provar que um estado corrompido é
    /// recusado é montar um estado **válido** e estragar um campo dele. Refazer as seções à mão
    /// faria o teste depender de todas as outras estarem certas, e ele passaria a medir outra coisa.
    pub fn troca(&mut self, nome: &str, bytes: Vec<u8>) {
        match self.pares.iter_mut().find(|(n, _)| n == nome) {
            Some((_, dados)) => *dados = bytes,
            None => self.poe(nome, bytes),
        }
    }

    /// Quantas seções já foram postas.
    pub fn quantas(&self) -> usize {
        self.pares.len()
    }

    /// Fecha o estado, no formato de [`escreve`].
    pub fn fecha(self) -> Vec<u8> {
        escreve(&self.pares)
    }
}

impl Leitor<'_> {
    /// Um número de uma seção, com erro que diz **qual** seção e o que faltou.
    ///
    /// Seção ausente é erro, e não zero: um campo que volta a zero em silêncio é a diferença entre
    /// "o jogo recomeça estranho" e "o carregamento foi recusado".
    pub fn u32(&self, nome: &str) -> Result<u32, Erro> {
        let bytes = self.secao(nome).ok_or_else(|| Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })?;
        if bytes.len() < 4 {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!("esperava 4 bytes e tem {}", bytes.len()),
            });
        }
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Uma lista de textos gravada por [`Secoes::poe_textos`].
    pub fn textos(&self, nome: &str) -> Result<Vec<String>, Erro> {
        let bytes = self.secao(nome).ok_or_else(|| Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })?;
        let malformada = |motivo: String| Erro::Secao {
            nome: nome.to_string(),
            motivo,
        };
        if bytes.len() < 4 {
            return Err(malformada(format!(
                "esperava a contagem e tem {} bytes",
                bytes.len()
            )));
        }
        let quantos = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let mut posicao = 4usize;
        let mut textos = Vec::with_capacity(quantos);
        for _ in 0..quantos {
            if bytes.len() < posicao + 4 {
                return Err(malformada(format!("falta o tamanho do texto em {posicao}")));
            }
            let tamanho = u32::from_le_bytes([
                bytes[posicao],
                bytes[posicao + 1],
                bytes[posicao + 2],
                bytes[posicao + 3],
            ]) as usize;
            posicao += 4;
            if bytes.len() < posicao + tamanho {
                return Err(malformada(format!(
                    "o texto em {posicao} diz ter {tamanho} bytes e restam {}",
                    bytes.len() - posicao
                )));
            }
            textos.push(
                String::from_utf8(bytes[posicao..posicao + tamanho].to_vec()).map_err(|erro| {
                    malformada(format!("um dos textos não é UTF-8: {erro}"))
                })?,
            );
            posicao += tamanho;
        }
        Ok(textos)
    }

    /// Blocos gravados por [`Secoes::poe_blocos`], com a contagem de chaves que o arquivo diz.
    ///
    /// O número de chaves mora **no arquivo**, e não só no código: assim uma tabela de chave dupla
    /// lida como se fosse de chave simples é recusa, e não uma leitura deslocada.
    pub fn blocos(&self, nome: &str) -> Result<Vec<(Vec<u32>, Vec<u8>)>, Erro> {
        let bytes = self.secao(nome).ok_or_else(|| Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })?;
        let malformada = |motivo: String| Erro::Secao {
            nome: nome.to_string(),
            motivo,
        };
        if bytes.len() < 8 {
            return Err(malformada(format!(
                "esperava a contagem e a largura da chave, e tem {} bytes",
                bytes.len()
            )));
        }
        let quantos = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let chaves = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        if chaves == 0 || chaves > 8 {
            return Err(malformada(format!("largura de chave {chaves}")));
        }
        let mut posicao = 8usize;
        let mut itens = Vec::with_capacity(quantos);
        for _ in 0..quantos {
            let precisa = posicao + chaves * 4 + 4;
            if bytes.len() < precisa {
                return Err(malformada(format!(
                    "o item em {posicao} passa do fim ({} bytes)",
                    bytes.len()
                )));
            }
            let mut chave = Vec::with_capacity(chaves);
            for _ in 0..chaves {
                chave.push(u32::from_le_bytes([
                    bytes[posicao],
                    bytes[posicao + 1],
                    bytes[posicao + 2],
                    bytes[posicao + 3],
                ]));
                posicao += 4;
            }
            let tamanho = u32::from_le_bytes([
                bytes[posicao],
                bytes[posicao + 1],
                bytes[posicao + 2],
                bytes[posicao + 3],
            ]) as usize;
            posicao += 4;
            if bytes.len() < posicao + tamanho {
                return Err(malformada(format!(
                    "o item {chave:?} diz ter {tamanho} bytes e restam {}",
                    bytes.len() - posicao
                )));
            }
            itens.push((chave, bytes[posicao..posicao + tamanho].to_vec()));
            posicao += tamanho;
        }
        Ok(itens)
    }

    /// Um texto gravado por [`Secoes::poe_texto`].
    ///
    /// Tamanho que passa do que o arquivo tem é recusa: cortar em silêncio devolveria um caminho
    /// pela metade, e o arquivo seria aberto no lugar errado.
    pub fn texto(&self, nome: &str) -> Result<String, Erro> {
        let bytes = self.secao(nome).ok_or_else(|| Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })?;
        if bytes.len() < 4 {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!("esperava o tamanho e tem {} bytes", bytes.len()),
            });
        }
        let quantos = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + quantos {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!(
                    "diz ter {quantos} bytes de texto e restam {}",
                    bytes.len() - 4
                ),
            });
        }
        String::from_utf8(bytes[4..4 + quantos].to_vec()).map_err(|erro| Erro::Secao {
            nome: nome.to_string(),
            motivo: format!("o texto não é UTF-8: {erro}"),
        })
    }

    /// Registros gravados por [`Secoes::poe_registros`], com o número de campos de cada um.
    ///
    /// O número de campos é **declarado por quem lê**, e um resto diferente de zero é recusa: um
    /// registro de quatro campos lido como se tivesse três deslocaria tudo o que vem depois, e o
    /// defeito apareceria como estado de outro jogo dentro do objeto errado.
    pub fn registros(&self, nome: &str, campos: usize) -> Result<Vec<Vec<u32>>, Erro> {
        assert!(campos > 0, "um registro sem campo nenhum não é registro");
        let valores = self.u32s(nome)?;
        if valores.len() % campos != 0 {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!(
                    "esperava múltiplo de {campos} valores e veio {}",
                    valores.len()
                ),
            });
        }
        Ok(valores.chunks_exact(campos).map(|r| r.to_vec()).collect())
    }

    /// Trios gravados por [`Secoes::poe_trios`].
    pub fn trios(&self, nome: &str) -> Result<Vec<(u32, u32, u32)>, Erro> {
        let valores = self.u32s(nome)?;
        if valores.len() % 3 != 0 {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!("esperava trios e veio {} valor(es)", valores.len()),
            });
        }
        Ok(valores
            .chunks_exact(3)
            .map(|t| (t[0], t[1], t[2]))
            .collect())
    }

    /// Pares **na ordem em que foram gravados**.
    ///
    /// É o que uma fila precisa: a ordem dos eventos é o conteúdo. Existe separado de
    /// [`Leitor::pares`], que ordena — ordenar serve para mapa, onde a ordem do `HashMap` não é
    /// estável, e **destrói** uma fila. Um save state que embaralha a fila de teclas entrega as
    /// teclas na ordem errada, e o jogo responde a uma sequência que ninguém apertou.
    pub fn pares_em_ordem(&self, nome: &str) -> Result<Vec<(u32, u32)>, Erro> {
        let valores = self.u32s(nome)?;
        if valores.len() % 2 != 0 {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!("esperava pares e veio ímpar ({})", valores.len()),
            });
        }
        Ok(valores.chunks_exact(2).map(|p| (p[0], p[1])).collect())
    }

    /// Pares de uma lista gravada por [`Secoes::poe_u32s`], para os mapas.
    ///
    /// **Ordena** os pares: a ordem de um `HashMap` não é estável entre execuções, e um estado
    /// que sai diferente a cada gravação não serve para comparar duas execuções.
    pub fn pares(&self, nome: &str) -> Result<Vec<(u32, u32)>, Erro> {
        let valores = self.u32s(nome)?;
        if valores.len() % 2 != 0 {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!("esperava pares e veio ímpar ({})", valores.len()),
            });
        }
        let mut pares: Vec<(u32, u32)> = valores.chunks_exact(2).map(|p| (p[0], p[1])).collect();
        pares.sort_unstable();
        Ok(pares)
    }

    /// Uma lista de números gravada por [`Secoes::poe_u32s`].
    pub fn u32s(&self, nome: &str) -> Result<Vec<u32>, Erro> {
        let bytes = self.secao(nome).ok_or_else(|| Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })?;
        if bytes.len() < 4 {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!("esperava a contagem e tem {} bytes", bytes.len()),
            });
        }
        let quantos = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let esperado = 4 + quantos * 4;
        if bytes.len() < esperado {
            return Err(Erro::Secao {
                nome: nome.to_string(),
                motivo: format!(
                    "diz ter {quantos} valores ({esperado} bytes) e tem {}",
                    bytes.len()
                ),
            });
        }
        Ok((0..quantos)
            .map(|i| {
                let p = 4 + i * 4;
                u32::from_le_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]])
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secoes() -> Vec<(String, Vec<u8>)> {
        vec![
            ("cpu".to_string(), vec![1, 2, 3, 4]),
            ("heap".to_string(), vec![9; 8]),
        ]
    }

    /// Um pedaço de estado de mentira, para exercitar o contrato `Guardavel` sem arrastar o motor.
    #[derive(Debug, PartialEq, Eq)]
    struct Contador {
        proximo: u32,
        livres: Vec<u32>,
    }

    impl Guardavel for Contador {
        fn grava(&self, destino: &mut Secoes) {
            destino.poe_u32("contador.proximo", self.proximo);
            destino.poe_u32s("contador.livres", self.livres.iter().copied());
        }

        fn restaura(&mut self, origem: &Leitor<'_>) -> Result<(), Erro> {
            self.proximo = origem.u32("contador.proximo")?;
            self.livres = origem.u32s("contador.livres")?;
            Ok(())
        }
    }

    #[test]
    fn um_guardavel_vai_e_volta() {
        let antes = Contador {
            proximo: 0x1020_3040,
            livres: vec![7, 0xabc, 0],
        };
        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        assert_eq!(secoes.quantas(), 2);
        let arquivo = secoes.fecha();

        let leitor = Leitor::abre(&arquivo).expect("o estado abriu");
        let mut depois = Contador {
            proximo: 0,
            livres: Vec::new(),
        };
        depois.restaura(&leitor).expect("restaurou");
        assert_eq!(depois, antes);
    }

    #[test]
    fn seçao_ausente_e_erro_e_nao_zero() {
        let mut secoes = Secoes::nova();
        secoes.poe_u32("so-isto", 1);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");
        match leitor.u32("nao-existe") {
            Err(Erro::Secao { nome, motivo }) => {
                assert_eq!(nome, "nao-existe");
                assert!(motivo.contains("não está"), "{motivo}");
            }
            outro => panic!("devia dizer que a seção não está, e devolveu {outro:?}"),
        }
    }

    /// **Ordenar serve para mapa; para fila, destrói.** Os dois leitores existem por isso, e este
    /// teste é a diferença entre eles escrita: um save state que embaralha a fila de teclas entrega
    /// as teclas na ordem errada, e o jogo responde a uma sequência que ninguém apertou.
    #[test]
    fn a_fila_nao_se_ordena_e_o_mapa_sim() {
        let mut secoes = Secoes::nova();
        // Uma fila de teclas fora de ordem crescente, de propósito.
        secoes.poe_u32s("fila", [30u32, 1, 20, 0]);
        secoes.poe_mapa("mapa", [(30u32, 1u32), (7, 2), (20, 3)]);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");

        assert_eq!(
            leitor.pares_em_ordem("fila"),
            Ok(vec![(30, 1), (20, 0)]),
            "a fila tem de sair na ordem em que foi gravada"
        );
        assert_eq!(
            leitor.pares("mapa"),
            Ok(vec![(7, 2), (20, 3), (30, 1)]),
            "o mapa sai ordenado, para não depender da ordem do HashMap"
        );
    }

    #[test]
    fn lista_de_numeros_com_contagem_mentirosa_e_recusada() {
        let mut secoes = Secoes::nova();
        // Contagem diz 5 valores e só vêm dois: o leitor tem de recusar, e não ler lixo.
        let mut bytes = 5u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0]);
        secoes.poe("mentirosa", bytes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");
        assert!(matches!(leitor.u32s("mentirosa"), Err(Erro::Secao { .. })));
    }

    #[test]
    fn a_ida_e_a_volta_devolvem_as_secoes() {
        let arquivo = escreve(&secoes());
        let leitor = Leitor::abre(&arquivo).expect("o estado abriu");
        assert_eq!(leitor.nomes(), vec!["cpu", "heap"]);
        assert_eq!(leitor.secao("cpu"), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(leitor.secao("heap"), Some(&[9u8; 8][..]));
        assert_eq!(leitor.secao("inexistente"), None);
    }

    #[test]
    fn uma_secao_desconhecida_e_pulada_sem_quebrar() {
        let mut todas = secoes();
        todas.push(("secao-do-futuro".to_string(), vec![7; 5]));
        let arquivo = escreve(&todas);
        let leitor = Leitor::abre(&arquivo).expect("o estado abriu");
        // As conhecidas continuam legíveis: é isto que deixa um motor velho ler um estado novo.
        assert_eq!(leitor.secao("cpu"), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(leitor.secao("secao-do-futuro"), Some(&[7u8; 5][..]));
    }

    #[test]
    fn um_byte_trocado_no_conteudo_e_recusado() {
        let mut arquivo = escreve(&secoes());
        let ultimo = arquivo.len() - 1;
        arquivo[ultimo] ^= 0xff;
        match Leitor::abre(&arquivo) {
            Err(Erro::Integridade { .. }) => {}
            outro => panic!("devia recusar por integridade, e devolveu {outro:?}"),
        }
    }

    #[test]
    fn uma_versao_do_futuro_e_recusada_dizendo_qual() {
        let mut arquivo = escreve(&secoes());
        arquivo[4..6].copy_from_slice(&(VERSAO + 3).to_le_bytes());
        match Leitor::abre(&arquivo) {
            Err(Erro::Versao {
                encontrada,
                suportada,
            }) => {
                assert_eq!(encontrada, VERSAO + 3);
                assert_eq!(suportada, VERSAO);
            }
            outro => panic!("devia recusar pela versão, e devolveu {outro:?}"),
        }
    }

    #[test]
    fn arquivo_de_outra_coisa_e_recusado() {
        match Leitor::abre(b"NOPE nao sou um save state") {
            Err(Erro::Assinatura(bytes)) => assert_eq!(&bytes, b"NOPE"),
            outro => panic!("devia recusar pela assinatura, e devolveu {outro:?}"),
        }
    }

    #[test]
    fn um_estado_cortado_e_recusado_com_os_dois_numeros() {
        let arquivo = escreve(&secoes());
        let cortado = &arquivo[..arquivo.len() - 3];
        match Leitor::abre(cortado) {
            Err(Erro::Truncado {
                esperado,
                encontrado,
            }) => {
                assert_eq!(esperado, arquivo.len());
                assert_eq!(encontrado, cortado.len());
            }
            outro => panic!("devia recusar por corte, e devolveu {outro:?}"),
        }
    }

    #[test]
    fn um_arquivo_vazio_e_recusado_sem_panico() {
        assert!(matches!(
            Leitor::abre(&[]),
            Err(Erro::Truncado {
                esperado: CABECALHO,
                encontrado: 0
            })
        ));
    }

    /// A mensagem de erro diz o que houve e com o que se deparou. Sem isto, quem recebe "save
    /// state inválido" começa a investigação do zero.
    #[test]
    fn as_mensagens_dizem_o_que_houve() {
        let texto = Erro::Versao {
            encontrada: 7,
            suportada: VERSAO,
        }
        .to_string();
        assert!(texto.contains('7'), "{texto}");
        assert!(texto.contains(&VERSAO.to_string()), "{texto}");
        let texto = Erro::Integridade {
            esperado: 0xdead_beef,
            encontrado: 0x1234_5678,
        }
        .to_string();
        assert!(texto.contains("deadbeef"), "{texto}");
        assert!(texto.contains("12345678"), "{texto}");
    }
}
