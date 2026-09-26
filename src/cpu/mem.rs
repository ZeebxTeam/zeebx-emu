//! Memória do guest.
//!
//! O mapa é montado por regiões nomeadas em vez de um bloco plano de 4 GiB: assim um acesso
//! fora do mapa vira erro identificável (e é justamente isso que o trampolim de vtables vai
//! usar para detectar chamadas a APIs do BREW).

// Removido assim que o núcleo estiver ligado ao loop principal.
#![allow(dead_code)]

use std::fmt;

/// Uma faixa contígua de memória do guest.
#[derive(Debug)]
pub struct Region {
    /// Nome usado em logs e mensagens de erro.
    pub name: &'static str,
    /// Endereço inicial no espaço do guest.
    pub base: u32,
    pub bytes: Vec<u8>,
    pub writable: bool,
    /// Se o processador pode buscar instruções aqui. Só a página nula não pode: ler dela é
    /// o que o console tolera, saltar para ela é erro que queremos ver.
    pub executavel: bool,
}

impl Region {
    fn contains(&self, addr: u32, len: u32) -> bool {
        let end = match self.base.checked_add(self.bytes.len() as u32) {
            Some(end) => end,
            None => return false,
        };
        addr >= self.base && addr.checked_add(len).is_some_and(|a| a <= end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemError {
    /// Nenhuma região cobre o endereço pedido.
    Unmapped { addr: u32, len: u32 },
    /// A região existe mas é somente leitura.
    ReadOnly { addr: u32, region: &'static str },
    /// Duas regiões se sobrepõem — erro de montagem do mapa, não do guest.
    Overlap { name: &'static str, base: u32 },
}

impl fmt::Display for MemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped { addr, len } => {
                write!(f, "acesso a {addr:#010x} (+{len}) fora do mapa de memória")
            }
            Self::ReadOnly { addr, region } => {
                write!(
                    f,
                    "escrita em {addr:#010x}, região '{region}' é somente leitura"
                )
            }
            Self::Overlap { name, base } => {
                write!(
                    f,
                    "região '{name}' em {base:#010x} sobrepõe outra já mapeada"
                )
            }
        }
    }
}

impl std::error::Error for MemError {}

#[derive(Debug, Default)]
pub struct GuestMemory {
    regions: Vec<Region>,
}

impl GuestMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mapeia uma nova região. Falha se ela colidir com alguma já existente.
    pub fn map(
        &mut self,
        name: &'static str,
        base: u32,
        bytes: Vec<u8>,
        writable: bool,
    ) -> Result<(), MemError> {
        self.map_com_execucao(name, base, bytes, writable, true)
    }

    /// Como [`GuestMemory::map`], dizendo também se a região pode ser executada.
    pub fn map_com_execucao(
        &mut self,
        name: &'static str,
        base: u32,
        bytes: Vec<u8>,
        writable: bool,
        executavel: bool,
    ) -> Result<(), MemError> {
        let end = base as u64 + bytes.len() as u64;
        let collides = self.regions.iter().any(|r| {
            let r_end = r.base as u64 + r.bytes.len() as u64;
            (base as u64) < r_end && (r.base as u64) < end
        });
        if collides {
            return Err(MemError::Overlap { name, base });
        }
        self.regions.push(Region {
            name,
            base,
            bytes,
            writable,
            executavel,
        });
        Ok(())
    }

    /// Mapeia uma região zerada e gravável — pilha, heap, framebuffer.
    pub fn map_zeroed(
        &mut self,
        name: &'static str,
        base: u32,
        len: usize,
    ) -> Result<(), MemError> {
        self.map(name, base, vec![0; len], true)
    }

    /// Se há código que possa ser buscado em `addr`.
    pub fn executavel(&self, addr: u32) -> bool {
        self.region_for(addr, 4).is_some_and(|r| r.executavel)
    }

    fn region_for(&self, addr: u32, len: u32) -> Option<&Region> {
        self.regions.iter().find(|r| r.contains(addr, len))
    }

    fn region_for_mut(&mut self, addr: u32, len: u32) -> Option<&mut Region> {
        self.regions.iter_mut().find(|r| r.contains(addr, len))
    }

    pub fn read(&self, addr: u32, len: u32) -> Result<&[u8], MemError> {
        let region = self
            .region_for(addr, len)
            .ok_or(MemError::Unmapped { addr, len })?;
        let start = (addr - region.base) as usize;
        Ok(&region.bytes[start..start + len as usize])
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemError> {
        let len = data.len() as u32;
        let region = self
            .region_for_mut(addr, len)
            .ok_or(MemError::Unmapped { addr, len })?;
        if !region.writable {
            return Err(MemError::ReadOnly {
                addr,
                region: region.name,
            });
        }
        let start = (addr - region.base) as usize;
        region.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// Preenche diretamente as regiões, sem criar um buffer intermediário.
    ///
    /// Valida o intervalo inteiro antes de tocar nos bytes. Assim uma falha na segunda região
    /// não deixa a primeira alterada sem o chamador invalidar o cache JIT correspondente.
    pub fn fill(&mut self, addr: u32, value: u8, len: u32) -> Result<(), MemError> {
        let inicio = u64::from(addr);
        let fim = inicio + u64::from(len);
        if fim > u64::from(u32::MAX) + 1 {
            return Err(MemError::Unmapped { addr, len });
        }

        let mut cursor = inicio;
        while cursor < fim {
            let onde = cursor as u32;
            let restante = fim - cursor;
            let region = self.region_for(onde, 1).ok_or(MemError::Unmapped {
                addr: onde,
                len: restante.min(u64::from(u32::MAX)) as u32,
            })?;
            if !region.writable {
                return Err(MemError::ReadOnly {
                    addr: onde,
                    region: region.name,
                });
            }
            let fim_da_regiao = u64::from(region.base) + region.bytes.len() as u64;
            cursor = fim.min(fim_da_regiao);
        }

        cursor = inicio;
        while cursor < fim {
            let onde = cursor as u32;
            let region = self.region_for_mut(onde, 1).expect("intervalo já validado");
            let inicio_na_regiao = (onde - region.base) as usize;
            let passo = ((fim - cursor) as usize).min(region.bytes.len() - inicio_na_regiao);
            region.bytes[inicio_na_regiao..inicio_na_regiao + passo].fill(value);
            cursor += passo as u64;
        }
        Ok(())
    }

    pub fn read_u32(&self, addr: u32) -> Result<u32, MemError> {
        let b = self.read(addr, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), MemError> {
        self.write(addr, &value.to_le_bytes())
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn regions_mut(&mut self) -> &mut [Region] {
        &mut self.regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> GuestMemory {
        let mut m = GuestMemory::new();
        m.map("rom", 0x0000_0000, vec![0xaa; 0x100], false).unwrap();
        m.map_zeroed("ram", 0x1000_0000, 0x100).unwrap();
        m
    }

    #[test]
    fn le_e_escreve_dentro_das_regioes() {
        let mut m = mem();
        assert_eq!(m.read(0, 2).unwrap(), &[0xaa, 0xaa]);
        m.write_u32(0x1000_0010, 0xdead_beef).unwrap();
        assert_eq!(m.read_u32(0x1000_0010).unwrap(), 0xdead_beef);
    }

    #[test]
    fn acesso_fora_do_mapa_falha() {
        let m = mem();
        assert_eq!(
            m.read(0x2000_0000, 4),
            Err(MemError::Unmapped {
                addr: 0x2000_0000,
                len: 4
            })
        );
    }

    #[test]
    fn acesso_que_cruza_o_fim_da_regiao_falha() {
        let m = mem();
        assert!(m.read(0xfe, 4).is_err());
    }

    #[test]
    fn escrita_em_regiao_somente_leitura_falha() {
        let mut m = mem();
        assert_eq!(
            m.write_u32(0, 1),
            Err(MemError::ReadOnly {
                addr: 0,
                region: "rom"
            })
        );
    }

    #[test]
    fn fill_preenche_ram_sem_buffer_intermediario() {
        let mut m = mem();
        m.fill(0x1000_0010, 0x5a, 0x20).unwrap();
        assert_eq!(m.read(0x1000_0010, 0x20).unwrap(), &[0x5a; 0x20]);
    }

    #[test]
    fn fill_respeita_regiao_somente_leitura() {
        let mut m = mem();
        assert_eq!(
            m.fill(0, 0, 4),
            Err(MemError::ReadOnly {
                addr: 0,
                region: "rom"
            })
        );
    }

    #[test]
    fn fill_que_falha_na_segunda_regiao_nao_altera_a_primeira() {
        let mut m = GuestMemory::new();
        m.map("w", 0x2000, vec![0xaa; 4], true).unwrap();
        m.map("ro", 0x2004, vec![0xbb; 4], false).unwrap();
        assert_eq!(
            m.fill(0x2002, 0, 4),
            Err(MemError::ReadOnly {
                addr: 0x2004,
                region: "ro"
            })
        );
        assert_eq!(m.read(0x2000, 4).unwrap(), &[0xaa; 4]);
    }
}
