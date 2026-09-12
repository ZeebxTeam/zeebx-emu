//! Abstração do núcleo ARM.
//!
//! O emulador nunca fala com um núcleo concreto: fala com [`CpuBackend`]. Hoje a implementação
//! prevista é o `unicorn-engine`; se a performance exigir, um backend sobre `dynarmic` (C++, via
//! FFI) entra no lugar sem tocar no resto do código.

// Removido assim que o núcleo estiver ligado ao loop principal.
#![allow(dead_code)]

pub mod mem;

use crate::cpu::mem::GuestMemory;

/// Registradores que o despacho de API precisa ler e escrever.
///
/// Segue a AAPCS: argumentos em `r0..r3` e o resto na pilha, retorno em `r0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    R0,
    R1,
    R2,
    R3,
    /// `r4..r11` não entram na convenção de chamada, mas são onde o compilador guarda o `this`
    /// e as variáveis vivas — sem eles não dá para reconstruir o contexto de uma falha.
    R4,
    R5,
    R6,
    R7,
    R8,
    R9,
    R10,
    R11,
    R12,
    Sp,
    Lr,
    Pc,
}

/// Por que a execução parou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// O guest saltou para o intervalo reservado às vtables do BREW — é uma chamada de API.
    ApiCall { addr: u32 },
    /// O guest retornou para o endereço-sentinela colocado em `lr` na entrada.
    Returned,
    /// Acesso a memória não mapeada. `addr` é o endereço acessado, `pc` a instrução que tentou.
    MemoryFault { addr: u32, pc: u32 },
    /// Instrução inválida, SWI ou outra exceção do núcleo.
    Exception { pc: u32 },
    /// Estourou o orçamento de instruções da fatia.
    Budget,
}

pub trait CpuBackend {
    /// Prepara o núcleo para executar com este mapa de memória.
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError>;

    fn read_reg(&self, reg: Reg) -> u32;
    fn write_reg(&mut self, reg: Reg, value: u32);

    /// Executa a partir de `pc` até parar, gastando no máximo `max_instructions`.
    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError>;

    /// Lê memória do guest. Depois do `reset` a memória vive dentro do núcleo, então as
    /// implementações de API precisam passar por aqui em vez de consultar o [`GuestMemory`].
    /// Quantas instruções o guest já executou.
    ///
    /// É o relógio do emulador: o tempo que o jogo enxerga vem daqui, e não do host, para que
    /// duas execuções iguais deem o mesmo resultado.
    fn instructions(&self) -> u64;

    /// Arma um sinalizador de sujeira numa faixa: o hook o liga quando o guest escreve nela.
    ///
    /// Sem isto, descobrir se o jogo mexeu numa superfície exige **ler a faixa inteira e
    /// comparar byte a byte**. Medido na Z-Wheel: o `sync` do color buffer do pbuffer era
    /// chamado 93 mil vezes em treze segundos, e a leitura mais a comparação somavam seis
    /// segundos — mais de um terço de todo o tempo de API.
    ///
    /// O `id` identifica a faixa, e existe porque há **várias** ao mesmo tempo: o color buffer
    /// do pbuffer e uma por superfície do jogo. Armar de novo com o mesmo `id` troca a faixa de
    /// lugar, que é o que acontece quando um bitmap é reexposto com outro tamanho.
    ///
    /// O padrão responde "sempre sujo", que é exatamente o comportamento anterior: um backend
    /// que não saiba armar o hook continua correto, só não fica mais rápido.
    fn watch_dirty(&mut self, _id: u32, _base: u32, _len: u32) -> Result<(), CpuError> {
        Ok(())
    }

    /// Desarma a faixa de `id`. Sem isto, a superfície de um bitmap já liberado continuaria
    /// custando um hook em toda escrita do guest naquele endereço.
    fn unwatch_dirty(&mut self, _id: u32) {}

    /// Lê **e limpa** o sinalizador de `id`. `true` quando o guest pode ter escrito desde a
    /// última vez, e também quando não há faixa armada com esse `id` — na dúvida, sujo.
    fn take_dirty(&mut self, _id: u32) -> bool {
        true
    }

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError>;

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError>;

    fn read_u32(&self, addr: u32) -> Result<u32, CpuError> {
        let mut buf = [0u8; 4];
        self.read_mem(addr, &mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), CpuError> {
        self.write_mem(addr, &value.to_le_bytes())
    }

    /// Lê uma string terminada em zero. Para na primeira falha de acesso e no limite dado,
    /// para que um ponteiro corrompido não vire leitura infinita.
    /// Os bytes de uma string do guest, até o terminador — sem interpretar codificação.
    ///
    /// É esta a versão que as funções de string do C precisam. A conversão para `String` troca
    /// cada byte inválido em UTF-8 por `U+FFFD`, que ocupa **três** bytes: `strlen("\x89PNG")`
    /// devolveria 6 em vez de 4, e foi exatamente esse erro que fez o Bejeweled Twist rejeitar
    /// os próprios PNGs.
    fn read_cbytes(&self, addr: u32, max_len: usize) -> Vec<u8> {
        /// Quanto ler de uma vez. Cada leitura é uma chamada ao núcleo, e byte a byte isso
        /// custava caro onde mais dói: o `strtoul` pede até `MAX_STRING` bytes e o Turma da
        /// Mônica o chama cem mil vezes, o que dava centenas de milhões de chamadas só para
        /// converter números. Sessenta e quatro bytes cobrem a string curta típica numa
        /// leitura só.
        const BLOCO: usize = 64;

        let mut bytes = Vec::new();
        while bytes.len() < max_len {
            let quer = BLOCO.min(max_len - bytes.len());
            let base = addr + bytes.len() as u32;
            let mut buf = vec![0u8; quer];
            if self.read_mem(base, &mut buf).is_err() {
                // O bloco pode cruzar o fim da região mapeada, e aí a leitura inteira falha
                // mesmo havendo bytes válidos antes. Byte a byte só neste caso.
                for offset in 0..quer {
                    let mut byte = [0u8; 1];
                    if self.read_mem(base + offset as u32, &mut byte).is_err() || byte[0] == 0 {
                        return bytes;
                    }
                    bytes.push(byte[0]);
                }
                continue;
            }
            match buf.iter().position(|&b| b == 0) {
                Some(fim) => {
                    bytes.extend_from_slice(&buf[..fim]);
                    return bytes;
                }
                None => bytes.extend_from_slice(&buf),
            }
        }
        bytes
    }

    /// A mesma string, interpretada como texto.
    fn read_cstring(&self, addr: u32, max_len: usize) -> String {
        latin1_decode(&self.read_cbytes(addr, max_len))
    }
}

/// Interpreta bytes de uma string `char` do BREW como texto.
///
/// O `char` do BREW é ISO-8859-1: um byte, um caractere. Ler como UTF-8 destruía tudo que
/// tivesse acento — `0xE7` não é UTF-8 válido, virava `U+FFFD` e voltava para a memória do
/// jogo como três bytes de lixo. É por isso que a acentuação saía quebrada na tela do
/// Resident Evil 4 e do Double Dragon, que passam o texto por `strtowstr`.
///
/// O mapeamento é direto porque os 256 primeiros pontos do Unicode *são* o ISO-8859-1, e por
/// isso a ida e a volta nunca perdem byte nenhum.
pub fn latin1_decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// O caminho de volta: cada caractere vira um byte.
///
/// O que não couber em um byte não veio da memória do guest e não tem representação lá; vira
/// `?`, que é o que a libc faz com um caractere fora da página de código.
pub fn latin1_encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(c as u32).unwrap_or(b'?'))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuError(pub String);

impl std::fmt::Display for CpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "falha no núcleo ARM: {}", self.0)
    }
}

impl std::error::Error for CpuError {}

pub mod dynarmic;
pub mod unicorn;

#[cfg(test)]
mod tests {
    use super::*;

    /// O texto acentuado tem de sobreviver à ida e à volta byte a byte: é isso que o jogo
    /// guarda na memória dele.
    #[test]
    fn latin1_ida_e_volta_preserva_acento() {
        let bytes = b"Configura\xe7\xf5es";
        let text = latin1_decode(bytes);
        assert_eq!(text, "Configurações");
        assert_eq!(latin1_encode(&text), bytes);
    }

    #[test]
    fn latin1_cobre_todos_os_bytes() {
        let bytes: Vec<u8> = (1..=255).collect();
        assert_eq!(latin1_encode(&latin1_decode(&bytes)), bytes);
    }
}
