//! Leitura de GIF — o formato da animação de abertura da Z-Wheel.
//!
//! O jogo pede o `opening_low.gif` pelo `ISHELL_LoadResObject`, e sem ele não passa: quando a
//! carga falha, a `AnimationVideo_Form.c` imprime `Missing startup animated GIF` e devolve
//! erro — e o caminho de limpeza dela **entra em recursão infinita**, porque chama o campo
//! `+0x20` do próprio objeto, que aponta para a função de limpeza. No console essa carga nunca
//! falha, então o laço não existe; aqui ele aparecia como pilha estourada em `0x1177c`.
//!
//! Decodificar é nosso, como o resto da imagem neste emulador. O GIF é simples o bastante para
//! isso ser razoável: uma paleta, um LZW e um punhado de blocos.
//!
//! **Quadros viram uma tira horizontal.** É o que o resto do emulador já entende: uma imagem
//! com `frame_width` menor que a largura é uma sequência, e o `IIMAGE_DrawFrame` escolhe a
//! coluna. Assim a animação não precisa de caminho próprio.

/// Um GIF decodificado: a tira de quadros e o tamanho de cada um.
pub struct Gif {
    /// Largura de **um** quadro.
    pub largura: u32,
    pub altura: u32,
    /// Os quadros, cada um com `largura * altura` pixels em RGBA.
    pub quadros: Vec<Vec<[u8; 4]>>,
}

/// O que um quadro faz com a tela antes do próximo, do bloco de controle gráfico.
#[derive(Clone, Copy, PartialEq)]
enum Descarte {
    /// Deixa como está — e é também o que "não especificado" significa na prática.
    Manter,
    /// Volta ao fundo, que para nós é transparente.
    Fundo,
    /// Volta ao que havia antes deste quadro.
    Anterior,
}

/// Um leitor de bytes que nunca estoura: acabou o arquivo, acabou a decodificação.
///
/// GIF é formato de rede, e arquivo truncado é coisa que acontece. Sem isto, cada leitura
/// precisaria do seu próprio `get()?`, e o erro apareceria como pânico em vez de "não deu".
struct Fita<'a> {
    bytes: &'a [u8],
    em: usize,
}

impl<'a> Fita<'a> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.em)?;
        self.em += 1;
        Some(b)
    }

    fn palavra(&mut self) -> Option<u16> {
        Some(u16::from(self.byte()?) | u16::from(self.byte()?) << 8)
    }

    fn pedaco(&mut self, quantos: usize) -> Option<&'a [u8]> {
        let saida = self.bytes.get(self.em..self.em + quantos)?;
        self.em += quantos;
        Some(saida)
    }

    /// Junta os sub-blocos até o de tamanho zero, que é como o GIF termina toda lista.
    fn sub_blocos(&mut self) -> Option<Vec<u8>> {
        let mut saida = Vec::new();
        loop {
            let n = self.byte()? as usize;
            if n == 0 {
                return Some(saida);
            }
            saida.extend_from_slice(self.pedaco(n)?);
        }
    }

    /// Pula os sub-blocos sem guardá-los.
    fn pula_sub_blocos(&mut self) -> Option<()> {
        self.sub_blocos().map(|_| ())
    }
}

/// Lê uma tabela de cores de `quantas` entradas, três bytes cada.
fn paleta(fita: &mut Fita, quantas: usize) -> Option<Vec<[u8; 3]>> {
    let cru = fita.pedaco(quantas * 3)?;
    Some(cru.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// Decodifica o GIF, ou devolve `None` se ele não for um.
pub fn decodifica(bytes: &[u8]) -> Option<Gif> {
    let mut fita = Fita { bytes, em: 0 };
    let assinatura = fita.pedaco(6)?;
    if assinatura != b"GIF87a" && assinatura != b"GIF89a" {
        return None;
    }
    let largura = fita.palavra()? as usize;
    let altura = fita.palavra()? as usize;
    if largura == 0 || altura == 0 {
        return None;
    }
    let campos = fita.byte()?;
    let _fundo = fita.byte()?;
    let _proporcao = fita.byte()?;
    let paleta_global = match campos & 0x80 != 0 {
        true => paleta(&mut fita, 2 << (campos & 7))?,
        false => Vec::new(),
    };

    // A tela começa toda transparente. Um GIF cujo primeiro quadro não cobre tudo conta com
    // isso — e o alfa zero é o que o resto do emulador entende por "não desenhe este pixel".
    let mut tela = vec![[0u8; 4]; largura * altura];
    let mut quadros: Vec<Vec<[u8; 4]>> = Vec::new();
    let (mut transparente, mut descarte) = (None, Descarte::Manter);

    loop {
        match fita.byte()? {
            // Fim do arquivo.
            0x3b => break,
            // Extensão. Só a de controle gráfico nos interessa; as outras são comentário,
            // texto e aplicação, e todas terminam em sub-blocos.
            0x21 => {
                let tipo = fita.byte()?;
                if tipo == 0xf9 {
                    let tamanho = fita.byte()? as usize;
                    let bloco = fita.pedaco(tamanho)?;
                    let campos = *bloco.first()?;
                    transparente = (campos & 1 != 0).then(|| *bloco.get(3).unwrap_or(&0));
                    descarte = match (campos >> 2) & 7 {
                        2 => Descarte::Fundo,
                        3 => Descarte::Anterior,
                        _ => Descarte::Manter,
                    };
                    fita.pula_sub_blocos()?;
                } else {
                    fita.pula_sub_blocos()?;
                }
            }
            // Um quadro.
            0x2c => {
                let esquerda = fita.palavra()? as usize;
                let topo = fita.palavra()? as usize;
                let largura_q = fita.palavra()? as usize;
                let altura_q = fita.palavra()? as usize;
                let campos = fita.byte()?;
                let cores = match campos & 0x80 != 0 {
                    true => paleta(&mut fita, 2 << (campos & 7))?,
                    false => paleta_global.clone(),
                };
                let entrelacado = campos & 0x40 != 0;
                let indices = descomprime(&mut fita, largura_q * altura_q)?;

                let salvo = (descarte == Descarte::Anterior).then(|| tela.clone());
                pinta(
                    &mut tela,
                    largura,
                    altura,
                    (esquerda, topo, largura_q, altura_q),
                    &indices,
                    &cores,
                    transparente,
                    entrelacado,
                );
                quadros.push(tela.clone());
                match descarte {
                    Descarte::Manter => {}
                    Descarte::Fundo => limpa(
                        &mut tela,
                        largura,
                        altura,
                        (esquerda, topo, largura_q, altura_q),
                    ),
                    Descarte::Anterior => {
                        if let Some(salvo) = salvo {
                            tela = salvo;
                        }
                    }
                }
                // O controle gráfico vale para **um** quadro só.
                transparente = None;
                descarte = Descarte::Manter;
            }
            // Byte que não abre bloco nenhum: arquivo estragado, e parar aqui é melhor do que
            // seguir interpretando lixo. O que já foi lido continua valendo.
            _ => break,
        }
    }

    (!quadros.is_empty()).then(|| Gif {
        largura: largura as u32,
        altura: altura as u32,
        quadros,
    })
}

/// Escreve os índices decodificados na tela, respeitando recorte, transparência e entrelace.
#[allow(clippy::too_many_arguments)]
fn pinta(
    tela: &mut [[u8; 4]],
    largura: usize,
    altura: usize,
    (esquerda, topo, largura_q, altura_q): (usize, usize, usize, usize),
    indices: &[u8],
    cores: &[[u8; 3]],
    transparente: Option<u8>,
    entrelacado: bool,
) {
    for i in 0..altura_q.min(indices.len() / largura_q.max(1)) {
        // No GIF entrelaçado as linhas vêm em quatro passadas, e a i-ésima linha **lida** não é
        // a i-ésima linha da imagem. Ignorar isso não quebra a decodificação: só embaralha a
        // imagem em faixas, que é um defeito discreto e difícil de reconhecer depois.
        let linha = match entrelacado {
            true => linha_entrelacada(i, altura_q),
            false => i,
        };
        for coluna in 0..largura_q {
            let indice = indices[i * largura_q + coluna];
            if Some(indice) == transparente {
                continue;
            }
            let (x, y) = (esquerda + coluna, topo + linha);
            if x >= largura || y >= altura {
                continue;
            }
            let cor = cores.get(indice as usize).copied().unwrap_or([0, 0, 0]);
            tela[y * largura + x] = [cor[0], cor[1], cor[2], 0xff];
        }
    }
}

/// Qual linha da imagem é a `n`-ésima linha lida, num GIF entrelaçado.
///
/// São quatro passadas: as linhas 0, 8, 16…; depois 4, 12, 20…; depois 2, 6, 10…; depois as
/// ímpares.
fn linha_entrelacada(n: usize, altura: usize) -> usize {
    let oitavos = altura.div_ceil(8);
    let quartos = altura.saturating_sub(4).div_ceil(8);
    let segundos = altura.saturating_sub(2).div_ceil(4);
    if n < oitavos {
        return n * 8;
    }
    if n < oitavos + quartos {
        return (n - oitavos) * 8 + 4;
    }
    if n < oitavos + quartos + segundos {
        return (n - oitavos - quartos) * 4 + 2;
    }
    (n - oitavos - quartos - segundos) * 2 + 1
}

/// Apaga o retângulo do quadro, deixando-o transparente.
fn limpa(
    tela: &mut [[u8; 4]],
    largura: usize,
    altura: usize,
    (esquerda, topo, largura_q, altura_q): (usize, usize, usize, usize),
) {
    for y in topo..(topo + altura_q).min(altura) {
        for x in esquerda..(esquerda + largura_q).min(largura) {
            tela[y * largura + x] = [0; 4];
        }
    }
}

/// O LZW do GIF, que não é o do TIFF nem o do `compress`.
///
/// O dicionário começa com uma entrada por cor, mais o código de recomeço e o de fim. Cada
/// código novo é "o anterior mais o primeiro byte do atual", e o tamanho do código cresce
/// sozinho quando o dicionário enche — até doze bits, onde ele para e espera um recomeço.
fn descomprime(fita: &mut Fita, quantos: usize) -> Option<Vec<u8>> {
    let bits_iniciais = fita.byte()? as u32;
    if !(1..=11).contains(&bits_iniciais) {
        return None;
    }
    let dados = fita.sub_blocos()?;

    let recomeco = 1u32 << bits_iniciais;
    let fim = recomeco + 1;
    // Cada entrada é `(anterior, byte)`; as primeiras são as cores, sem anterior.
    let inicial: Vec<(u32, u8)> = (0..recomeco).map(|c| (u32::MAX, c as u8)).collect();
    let mut tabela = inicial.clone();
    let mut bits = bits_iniciais + 1;
    let mut anterior: Option<u32> = None;

    let mut saida = Vec::with_capacity(quantos);
    let (mut acumulado, mut quantos_bits, mut em) = (0u32, 0u32, 0usize);
    let mut sequencia = Vec::new();

    while saida.len() < quantos {
        while quantos_bits < bits {
            let Some(&b) = dados.get(em) else {
                // Fluxo truncado: entrega o que deu, porque meia imagem ainda é imagem, e o
                // chamador não tem como saber a diferença de outra forma.
                return Some(saida);
            };
            acumulado |= u32::from(b) << quantos_bits;
            quantos_bits += 8;
            em += 1;
        }
        let codigo = acumulado & ((1 << bits) - 1);
        acumulado >>= bits;
        quantos_bits -= bits;

        if codigo == recomeco {
            tabela.clone_from(&inicial);
            tabela.push((u32::MAX, 0));
            tabela.push((u32::MAX, 0));
            bits = bits_iniciais + 1;
            anterior = None;
            continue;
        }
        if codigo == fim {
            break;
        }

        // O caso em que o código ainda não está na tabela é legítimo, e é o que faz o LZW
        // apertar bem sequências repetidas: ele significa "o anterior mais o primeiro byte do
        // anterior". Tratá-lo como erro estraga imagens perfeitamente válidas.
        let novo_primeiro;
        sequencia.clear();
        if (codigo as usize) < tabela.len() {
            desenrola(&tabela, codigo, &mut sequencia);
            novo_primeiro = *sequencia.first()?;
        } else {
            let anterior = anterior?;
            desenrola(&tabela, anterior, &mut sequencia);
            novo_primeiro = *sequencia.first()?;
            sequencia.push(novo_primeiro);
        }
        saida.extend_from_slice(&sequencia);

        if let Some(anterior) = anterior
            && tabela.len() < 4096
        {
            tabela.push((anterior, novo_primeiro));
            if tabela.len() == (1 << bits) && bits < 12 {
                bits += 1;
            }
        }
        anterior = Some(codigo);
    }
    Some(saida)
}

/// Monta a sequência de bytes de um código, seguindo a corrente de prefixos.
fn desenrola(tabela: &[(u32, u8)], codigo: u32, saida: &mut Vec<u8>) {
    let mut atual = codigo;
    // O teto é o tamanho da tabela: uma corrente mais longa que isso só existe se ela tiver
    // ciclo, e aí o que se ganha em parar é não travar o emulador com um arquivo estragado.
    for _ in 0..tabela.len() {
        let Some(&(anterior, byte)) = tabela.get(atual as usize) else {
            break;
        };
        saida.push(byte);
        if anterior == u32::MAX {
            break;
        }
        atual = anterior;
    }
    saida.reverse();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Um GIF de 2×2 com quatro cores, escrito à mão, sem compressão real: cada pixel sai como
    /// um código literal, com recomeço na frente e fim atrás.
    fn gif_de_quatro_pixels() -> Vec<u8> {
        let mut g = Vec::new();
        g.extend_from_slice(b"GIF89a");
        g.extend_from_slice(&2u16.to_le_bytes()); // largura
        g.extend_from_slice(&2u16.to_le_bytes()); // altura
        g.push(0x80 | 1); // paleta global de quatro cores
        g.push(0);
        g.push(0);
        g.extend_from_slice(&[0xff, 0, 0]); // 0 vermelho
        g.extend_from_slice(&[0, 0xff, 0]); // 1 verde
        g.extend_from_slice(&[0, 0, 0xff]); // 2 azul
        g.extend_from_slice(&[0xff, 0xff, 0xff]); // 3 branco
        g.push(0x2c);
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&2u16.to_le_bytes());
        g.extend_from_slice(&2u16.to_le_bytes());
        g.push(0); // sem paleta local, sem entrelace
        g.push(2); // dois bits por código inicial
        // Recomeço(4), os quatro literais, fim(5) — **com a largura crescendo no meio**.
        //
        // O dicionário começa com seis entradas (quatro cores, recomeço e fim) e ganha uma por
        // código lido a partir do segundo. Quando ele chega a oito, a largura passa de três
        // para quatro bits — e isso vale já para o **próximo** código. Escrever tudo com três
        // bits, como eu fiz na primeira versão deste teste, produz um arquivo que decodificador
        // nenhum lê: o teste falhava acusando o decodificador, que estava certo.
        let codigos: [(u32, u32); 6] = [(4, 3), (0, 3), (1, 3), (2, 3), (3, 4), (5, 4)];
        let (mut acc, mut n, mut fluxo) = (0u32, 0u32, Vec::new());
        for (c, largura) in codigos {
            acc |= c << n;
            n += largura;
            while n >= 8 {
                fluxo.push((acc & 0xff) as u8);
                acc >>= 8;
                n -= 8;
            }
        }
        if n > 0 {
            fluxo.push(acc as u8);
        }
        g.push(fluxo.len() as u8);
        g.extend_from_slice(&fluxo);
        g.push(0);
        g.push(0x3b);
        g
    }

    #[test]
    fn le_os_quatro_pixels_na_ordem_certa() {
        let gif = decodifica(&gif_de_quatro_pixels()).expect("devia decodificar");
        assert_eq!((gif.largura, gif.altura), (2, 2));
        assert_eq!(gif.quadros.len(), 1);
        assert_eq!(
            gif.quadros[0],
            vec![
                [0xff, 0, 0, 0xff],
                [0, 0xff, 0, 0xff],
                [0, 0, 0xff, 0xff],
                [0xff, 0xff, 0xff, 0xff],
            ]
        );
    }

    #[test]
    fn recusa_o_que_nao_e_gif() {
        assert!(decodifica(b"PNG\x00qualquercoisa").is_none());
        assert!(decodifica(b"").is_none());
    }

    /// Truncar em qualquer ponto tem de devolver `None` ou uma imagem — nunca pânico. Um
    /// recurso estragado no pacote de um jogo não pode derrubar o emulador.
    #[test]
    fn arquivo_truncado_nao_derruba() {
        let inteiro = gif_de_quatro_pixels();
        for ate in 0..inteiro.len() {
            let _ = decodifica(&inteiro[..ate]);
        }
    }

    #[test]
    fn o_entrelace_percorre_as_quatro_passadas() {
        let lidas: Vec<usize> = (0..8).map(|n| linha_entrelacada(n, 8)).collect();
        assert_eq!(lidas, vec![0, 4, 2, 6, 1, 3, 5, 7]);
    }
}
