//! As funções de apoio do BREW: o printf, o qsort e a conversão de string.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `qsort` da stdlib do BREW, com a comparação feita pelo jogo.
    ///
    /// Ordena **na memória do guest**, trocando os elementos de lugar de verdade, para que os
    /// ponteiros que a função de comparação recebe sejam os endereços reais dentro do vetor —
    /// que é o que o `qsort` do C faz e o que um comparador pode observar.
    ///
    /// O algoritmo é o heapsort: ordena no lugar, sem memória extra, e faz `n log n`
    /// comparações. Como cada comparação custa uma entrada no guest, o número delas é o que
    /// importa aqui — uma ordenação por inserção seria simples mas quadrática, e um vetor
    /// grande custaria caro.
    pub(super) fn qsort(
        &mut self,
        base: u32,
        count: u32,
        size: u32,
        compare: u32,
    ) -> Result<(), CpuError> {
        if base == 0 || compare == 0 || size == 0 || count < 2 {
            return Ok(());
        }
        // Reentrar no guest exige espaço de aninhamento, como nos callbacks.
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("um qsort foi ignorado por aninhamento profundo demais");
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let result = self.heapsort(base, count as usize, size, compare);
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        result
    }

    pub(super) fn heapsort(
        &mut self,
        base: u32,
        count: usize,
        size: u32,
        compare: u32,
    ) -> Result<(), CpuError> {
        for start in (0..count / 2).rev() {
            self.sift_down(base, size, compare, start, count)?;
        }
        for end in (1..count).rev() {
            self.swap_elements(base, size, 0, end)?;
            self.sift_down(base, size, compare, 0, end)?;
        }
        Ok(())
    }

    /// Empurra o elemento em `root` para baixo até o monte voltar a valer.
    pub(super) fn sift_down(
        &mut self,
        base: u32,
        size: u32,
        compare: u32,
        mut root: usize,
        end: usize,
    ) -> Result<(), CpuError> {
        loop {
            let child = root * 2 + 1;
            if child >= end {
                return Ok(());
            }
            let mut largest = child;
            if child + 1 < end && self.compare_elements(base, size, compare, child, child + 1)? < 0
            {
                largest = child + 1;
            }
            if self.compare_elements(base, size, compare, root, largest)? >= 0 {
                return Ok(());
            }
            self.swap_elements(base, size, root, largest)?;
            root = largest;
        }
    }

    /// Chama a função de comparação do jogo com os endereços dos dois elementos.
    pub(super) fn compare_elements(
        &mut self,
        base: u32,
        size: u32,
        compare: u32,
        a: usize,
        b: usize,
    ) -> Result<i32, CpuError> {
        let (pa, pb) = (base + a as u32 * size, base + b as u32 * size);
        let outcome = self.call_guest(compare, [pa, pb, 0, 0], QSORT_BUDGET)?;
        match outcome {
            // Uma comparação que não retorna deixa a ordem como está, em vez de derrubar tudo.
            Outcome::Returned { code } => Ok(code as i32),
            _ => Ok(0),
        }
    }

    pub(super) fn swap_elements(
        &mut self,
        base: u32,
        size: u32,
        a: usize,
        b: usize,
    ) -> Result<(), CpuError> {
        if a == b {
            return Ok(());
        }
        let (pa, pb) = (base + a as u32 * size, base + b as u32 * size);
        let first = self.read_bytes(pa, size)?;
        let second = self.read_bytes(pb, size)?;
        self.cpu.write_mem(pa, &second)?;
        self.cpu.write_mem(pb, &first)?;
        Ok(())
    }

    /// Argumento além dos quatro registradores, contado a partir do topo da pilha.
    pub(super) fn stack_arg(&self, index: u32) -> Result<u32, CpuError> {
        self.cpu.read_u32(self.cpu.read_reg(Reg::Sp) + index * 4)
    }

    /// Lê uma string `AECHAR` — UTF-16 little-endian terminada em zero, que é como o BREW
    /// representa texto.
    /// Lê uma string larga do guest como as unidades UTF-16 cruas.
    ///
    /// Existe separada da versão em `String` porque `wstrchr` e as comparações trabalham em
    /// cima de `AECHAR`, e converter para `String` perderia a posição de cada caractere.
    pub(super) fn read_aechar_units(&self, addr: u32) -> Result<Vec<u16>, CpuError> {
        if addr == 0 {
            return Ok(Vec::new());
        }
        /// Quantas unidades por leitura. Mesmo motivo do `read_cbytes`: cada leitura
        /// atravessa a FFI do unicorn, que procura a região antes de copiar — 57 ns para
        /// trazer dois bytes. Sessenta e quatro unidades cobrem a string típica de uma vez.
        const BLOCO: usize = 64;

        let mut units = Vec::new();
        while units.len() < MAX_STRING {
            let quer = BLOCO.min(MAX_STRING - units.len());
            let base = addr + units.len() as u32 * 2;
            let mut bytes = vec![0u8; quer * 2];
            if self.cpu.read_mem(base, &mut bytes).is_err() {
                // O bloco pode cruzar o fim da região mapeada, e aí a leitura inteira falha
                // mesmo havendo unidades válidas antes. Duas em duas só neste caso.
                for i in 0..quer {
                    let mut par = [0u8; 2];
                    if self.cpu.read_mem(base + i as u32 * 2, &mut par).is_err() {
                        return Ok(units);
                    }
                    match u16::from_le_bytes(par) {
                        0 => return Ok(units),
                        unit => units.push(unit),
                    }
                }
                continue;
            }
            for par in bytes.chunks_exact(2) {
                match u16::from_le_bytes([par[0], par[1]]) {
                    0 => return Ok(units),
                    unit => units.push(unit),
                }
            }
        }
        Ok(units)
    }

    pub(super) fn read_aechar(&self, addr: u32) -> Result<String, CpuError> {
        // Era o mesmo laço do `read_aechar_units` escrito de novo. A única diferença é o
        // que se faz com as unidades no fim.
        Ok(String::from_utf16_lossy(&self.read_aechar_units(addr)?))
    }

    /// Helpers da stdlib do BREW, despachados pelo nome do slot — a tabela vem de
    /// `struct AEEHelperFuncs`, no `AEEStdLib.h` do SDK.
    pub(super) fn helper_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Helpers.method(slot) else {
            return Ok(None);
        };
        let (a0, a1, a2) = (
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
        );
        let result = match name {
            // void *SetupNativeImage(AEECLSID cls, void *pBuffer, AEEImageInfo *pii,
            //                         boolean *pbRealloc)
            //
            // Converte uma imagem codificada para o formato nativo do aparelho. É o que está
            // por trás do `CONVERTBMP` do SDK, e os dois jogos que a chamam passam
            // `AEECLSID_WINBMP` com um bitmap do Windows.
            "SetupNativeImage" => self.setup_native_image(a1, a2, self.cpu.read_reg(Reg::R3))?,
            "malloc" => {
                let r = self.malloc(a0)?;
                r
            }
            "free" => {
                self.heap.free(a0);
                SUCCESS
            }
            "realloc" => self.realloc(a0, a1 & !ALLOC_NO_ZMEM)?,
            "memmove" => {
                self.copy_guest(a0, a1, a2)?;
                a0
            }
            "memset" => {
                if a2 > 0 {
                    self.cpu.write_mem(a0, &vec![a1 as u8; a2 as usize])?;
                }
                a0
            }
            "memcmp" => {
                let (left, right) = (self.read_bytes(a0, a2)?, self.read_bytes(a1, a2)?);
                cmp_to_int(left.cmp(&right))
            }
            // As funções de string do C contam **bytes**, não caracteres: qualquer byte acima
            // de 0x7f faria a versão em `String` mentir.
            "strlen" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32,
            "strcpy" => {
                let mut src = self.cpu.read_cbytes(a1, MAX_STRING);
                src.push(0);
                self.cpu.write_mem(a0, &src)?;
                a0
            }
            "strcat" => {
                let base = self.cpu.read_cbytes(a0, MAX_STRING).len() as u32;
                let mut extra = self.cpu.read_cbytes(a1, MAX_STRING);
                extra.push(0);
                self.cpu.write_mem(a0 + base, &extra)?;
                a0
            }
            "strcmp" => {
                let left = self.cpu.read_cbytes(a0, MAX_STRING);
                let right = self.cpu.read_cbytes(a1, MAX_STRING);
                cmp_to_int(left.cmp(&right))
            }
            "strncmp" => {
                let take = a2 as usize;
                let left = self.cpu.read_cbytes(a0, MAX_STRING);
                let right = self.cpu.read_cbytes(a1, MAX_STRING);
                // `strncmp` compara no máximo `n` bytes; uma string mais curta vale inteira.
                cmp_to_int(left[..take.min(left.len())].cmp(&right[..take.min(right.len())]))
            }
            "wstrlen" => self.read_aechar(a0)?.encode_utf16().count() as u32,
            "wstrsize" => (self.read_aechar(a0)?.encode_utf16().count() as u32 + 1) * 2,
            // AECHAR *strtowstr(const char *pszIn, AECHAR *pDest, int nSize) — `nSize` é em
            // bytes, não em caracteres.
            "strtowstr" => {
                let text = self.cpu.read_cstring(a0, MAX_STRING);
                self.write_aechar(a1, &text, a2 as usize / 2)?;
                a1
            }
            // char *wstrtostr(const AECHAR *pIn, char *pszDest, int nSize)
            "wstrtostr" => {
                let text = self.read_aechar(a0)?;
                self.write_cstring_limited(a1, &text, a2 as usize)?;
                a1
            }
            "wstrcpy" => {
                let text = self.read_aechar(a1)?;
                self.write_aechar(a0, &text, usize::MAX)?;
                a0
            }
            "wstrcat" => {
                let base = self.read_aechar(a0)?;
                let extra = self.read_aechar(a1)?;
                let offset = base.encode_utf16().count() as u32 * 2;
                self.write_aechar(a0 + offset, &extra, usize::MAX)?;
                a0
            }
            "wstrcmp" => {
                let (left, right) = (self.read_aechar(a0)?, self.read_aechar(a1)?);
                cmp_to_int(left.cmp(&right))
            }
            "wstrncmp" => {
                let take = a2 as usize;
                let (left, right) = (self.read_aechar(a0)?, self.read_aechar(a1)?);
                cmp_to_int(left.get(..take).cmp(&right.get(..take)))
            }
            // char *strncpy(char *dst, const char *src, size_t n) — sem terminador se `src`
            // não couber, como manda a função original.
            "strncpy" => {
                let n = a2 as usize;
                let mut bytes = self.cpu.read_cbytes(a1, MAX_STRING);
                bytes.truncate(n);
                bytes.resize(n, 0);
                if n > 0 {
                    self.cpu.write_mem(a0, &bytes)?;
                }
                a0
            }
            "strchr" | "strrchr" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = a1 as u8;
                let found = if name == "strchr" {
                    text.iter().position(|&b| b == needle)
                } else {
                    text.iter().rposition(|&b| b == needle)
                };
                found.map(|i| a0 + i as u32).unwrap_or(0)
            }
            // char *strstr(const char *s, const char *agulha)
            //
            // **Agulha vazia casa no começo.** É o que o C manda — toda string contém a string
            // vazia — e não é curiosidade de especificação: o Need For Speed registra os sons de
            // jogo procurando o nome de cada um numa tabela, e um dos cinco nomes dele é a
            // string vazia. Devolvendo "não achou", ele varria as setenta entradas, concluía que
            // o som não existe, imprimia `ZeeboSnd.cpp:327 BREAKPOINT!` e entrava num salto para
            // si mesmo — de propósito. O jogo não estava lento: estava parado, e o orçamento de
            // instruções acabava em cima disso.
            "strstr" => {
                let haystack = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = self.cpu.read_cbytes(a1, MAX_STRING);
                let achou = match needle.is_empty() {
                    true => a0,
                    false => haystack
                        .windows(needle.len())
                        .position(|w| w == needle)
                        .map(|i| a0 + i as u32)
                        .unwrap_or(0),
                };
                if self.serial.is_some() {
                    let n = String::from_utf8_lossy(&needle).into_owned();
                    self.registra_serial(format!("<strstr {n:?} -> {achou:#x}>"));
                }
                achou
            }
            "stricmp" => {
                let fold = |addr| {
                    self.cpu
                        .read_cbytes(addr, MAX_STRING)
                        .iter()
                        .map(u8::to_ascii_lowercase)
                        .collect::<Vec<_>>()
                };
                cmp_to_int(fold(a0).cmp(&fold(a1)))
            }
            "memchr" => {
                let bytes = self.read_bytes(a0, a2)?;
                bytes
                    .iter()
                    .position(|&b| b == a1 as u8)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // --- Strings largas (`AECHAR`, UTF-16) -------------------------------------
            "wstrchr" | "wstrrchr" => {
                let units = self.read_aechar_units(a0)?;
                let needle = a1 as u16;
                let found = if name == "wstrchr" {
                    units.iter().position(|&u| u == needle)
                } else {
                    units.iter().rposition(|&u| u == needle)
                };
                found.map(|i| a0 + i as u32 * 2).unwrap_or(0)
            }
            "wstrdup" => {
                let units = self.read_aechar_units(a0)?;
                let bytes: Vec<u8> = units
                    .iter()
                    .chain(std::iter::once(&0))
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            "wstrlower" | "wstrupper" => {
                let units = self.read_aechar_units(a0)?;
                let mapped: Vec<u8> = units
                    .iter()
                    .map(|&u| match u8::try_from(u) {
                        Ok(b) if name == "wstrlower" => b.to_ascii_lowercase() as u16,
                        Ok(b) => b.to_ascii_uppercase() as u16,
                        Err(_) => u,
                    })
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                if !mapped.is_empty() {
                    self.cpu.write_mem(a0, &mapped)?;
                }
                a0
            }
            "wstricmp" | "wstrnicmp" => {
                let take = if name == "wstricmp" {
                    usize::MAX
                } else {
                    a2 as usize
                };
                let left = self.read_aechar_units(a0)?;
                let right = self.read_aechar_units(a1)?;
                cmp_to_int(fold_case(&left, take).cmp(&fold_case(&right, take)))
            }
            // `size_t wstrlcpy/wstrlcat(AECHAR *dst, const AECHAR *src, size_t nSize)`: as
            // versões BSD, que devolvem o tamanho que a origem *teria* ocupado. `nSize` conta
            // caracteres, não bytes.
            "wstrlcpy" | "wstrlcat" => {
                let source = self.read_aechar(a1)?;
                let existing = if name == "wstrlcat" {
                    self.read_aechar_units(a0)?.len()
                } else {
                    0
                };
                let room = (a2 as usize).saturating_sub(existing);
                self.write_aechar(a0 + existing as u32 * 2, &source, room)?;
                (existing + source.encode_utf16().count()) as u32
            }
            // `AECHAR *wwritelong(AECHAR *pszBuf, long n)` — escreve o número e devolve o
            // ponteiro para o terminador, para o chamador continuar dali.
            "wwritelong" => {
                let text = (a1 as i32).to_string();
                self.write_aechar(a0, &text, usize::MAX)?;
                a0 + text.len() as u32 * 2
            }
            // `int wstrncopyn(AECHAR *dst, int cbDest, const AECHAR *src, int lenSource)`:
            // `cbDest` conta caracteres e `lenSource` limita a origem (-1 = até o terminador).
            "wstrncopyn" => {
                let mut units = self.read_aechar_units(a2)?;
                let limit = self.cpu.read_reg(Reg::R3) as i32;
                if limit >= 0 {
                    units.truncate(limit as usize);
                }
                let text = String::from_utf16_lossy(&units);
                self.write_aechar(a0, &text, a1 as usize)?;
                text.encode_utf16()
                    .count()
                    .min((a1 as usize).saturating_sub(1)) as u32
            }
            // `void wsprintf(AECHAR *dst, int nSize, const AECHAR *fmt, ...)` — mesma
            // gramática do `snprintf`, com origem e destino em UTF-16.
            "wsprintf" => {
                let fmt = self.read_aechar(a2)?;
                let text = self.format_from(3, &fmt);
                self.write_aechar(a0, &text, a1 as usize / 2)?;
                SUCCESS
            }
            // `void strexpand(const byte *pSrc, int nCount, AECHAR *pDest, int nSize)`
            "strexpand" => {
                let bytes = self.read_bytes(a0, a1)?;
                let text: String = bytes.iter().map(|&b| b as char).collect();
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &text, limit)?;
                SUCCESS
            }
            "wstrtofloat" => {
                let text = self.read_aechar(a0)?;
                let (value, _) = parse_leading_double(&text);
                self.return_double(value)
            }
            // `boolean floattowstr(double val, AECHAR *psz, int nSize)`: o `double` ocupa
            // `r0:r1`, então o destino cai em `r2` e o tamanho em `r3`.
            "floattowstr" => {
                let value = fmath::from_words(a0, a1);
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &format!("{value}"), limit)?;
                TRUE
            }
            // `boolean utf8towstr(const byte *pszIn, int nLen, AECHAR *pDest, int nSizeBytes)`
            "utf8towstr" => {
                let bytes = if (a1 as i32) < 0 {
                    self.cpu.read_cbytes(a0, MAX_STRING)
                } else {
                    self.read_bytes(a0, a1)?
                };
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &text, limit)?;
                TRUE
            }
            // `boolean wstrtoutf8(const AECHAR *pszIn, int nLen, byte *pDest, int nSizeBytes)`
            "wstrtoutf8" => {
                let mut units = self.read_aechar_units(a0)?;
                if (a1 as i32) >= 0 {
                    units.truncate(a1 as usize);
                }
                let text = String::from_utf16_lossy(&units);
                self.write_cstring_limited(a2, &text, self.cpu.read_reg(Reg::R3) as usize)?;
                TRUE
            }

            // --- Strings de bytes -----------------------------------------------------
            "strdup" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                bytes.push(0);
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            // void qsort(void *base, size_t nmemb, size_t size, int (*compar)(const void *,
            //            const void *))
            //
            // A comparação é uma função do **jogo**, então ordenar significa reentrar no guest
            // a cada par. É o mesmo desvio que os callbacks pendentes fazem, e no mesmo ponto:
            // a chamada de API terminou e o guest ainda não retomou.
            "qsort" => {
                let compare = self.cpu.read_reg(Reg::R3);
                self.qsort(a0, a1, a2, compare)?;
                SUCCESS
            }
            // `uint32 strtoul(const char *nptr, char **endptr, int base)`
            "strtoul" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                let (value, consumed) = parse_unsigned(&text, a2);
                if a1 != 0 {
                    self.cpu.write_u32(a1, a0 + consumed as u32)?;
                }
                value
            }
            "strnicmp" => {
                let take = a2 as usize;
                let left = self.cpu.read_cbytes(a0, MAX_STRING).to_ascii_lowercase();
                let right = self.cpu.read_cbytes(a1, MAX_STRING).to_ascii_lowercase();
                cmp_to_int(left[..take.min(left.len())].cmp(&right[..take.min(right.len())]))
            }
            "stristr" => {
                let hay = self.cpu.read_cbytes(a0, MAX_STRING).to_ascii_lowercase();
                let needle = self.cpu.read_cbytes(a1, MAX_STRING).to_ascii_lowercase();
                find_subslice(&hay, &needle)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // `char *memstr(const char *cpHaystack, const char *cpszNeedle, size_t nLen)` — o
            // palheiro tem tamanho fixo; a agulha continua terminada em NUL.
            "memstr" => {
                let hay = self.read_bytes(a0, a2)?;
                let needle = self.cpu.read_cbytes(a1, MAX_STRING);
                find_subslice(&hay, &needle)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // `boolean strbegins(const char *cpszPrefix, const char *psz)` e o par `strends`:
            // o pedaço procurado vem **primeiro**.
            "strbegins" | "strends" | "aee_stribegins" => {
                let part = self.cpu.read_cbytes(a0, MAX_STRING);
                let whole = self.cpu.read_cbytes(a1, MAX_STRING);
                let matched = match name {
                    "strbegins" => whole.starts_with(&part),
                    "strends" => whole.ends_with(&part),
                    _ => whole
                        .to_ascii_lowercase()
                        .starts_with(&part.to_ascii_lowercase()),
                };
                u32::from(matched)
            }
            // `char *strchrend(const char *pszSrc, char c)` — como o `strchr`, mas quando não
            // acha devolve o terminador em vez de zero.
            "strchrend" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = a1 as u8;
                let at = text.iter().position(|&b| b == needle).unwrap_or(text.len());
                a0 + at as u32
            }
            // `char *strchrsend(const char *pszSrc, const char *pszChars)` — o primeiro byte
            // que aparecer no conjunto, ou o terminador.
            "strchrsend" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let set = self.cpu.read_cbytes(a1, MAX_STRING);
                let at = text
                    .iter()
                    .position(|b| set.contains(b))
                    .unwrap_or(text.len());
                a0 + at as u32
            }
            // A família `mem*` do BREW trabalha sobre um bloco de tamanho fixo: `memrchr` acha
            // a última ocorrência, `memchrend`/`memrchrbegin` devolvem os limites do bloco
            // quando não acham.
            "memrchr" | "memchrend" | "memrchrbegin" => {
                let block = self.read_bytes(a0, a2)?;
                let needle = a1 as u8;
                let at = match name {
                    "memchrend" => block
                        .iter()
                        .position(|&b| b == needle)
                        .unwrap_or(block.len()),
                    "memrchrbegin" => block.iter().rposition(|&b| b == needle).unwrap_or_default(),
                    _ => match block.iter().rposition(|&b| b == needle) {
                        Some(i) => i,
                        None => return Ok(Some(0)),
                    },
                };
                a0 + at as u32
            }
            "strlower" | "strupper" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                if name == "strlower" {
                    bytes.make_ascii_lowercase();
                } else {
                    bytes.make_ascii_uppercase();
                }
                if !bytes.is_empty() {
                    self.cpu.write_mem(a0, &bytes)?;
                }
                a0
            }
            // `size_t strlcpy/strlcat(char *dst, const char *src, size_t nSize)`, à moda BSD.
            "strlcpy" | "strlcat" => {
                let source = self.cpu.read_cstring(a1, MAX_STRING);
                let existing = if name == "strlcat" {
                    self.cpu.read_cbytes(a0, MAX_STRING).len()
                } else {
                    0
                };
                let room = (a2 as usize).saturating_sub(existing);
                self.write_cstring_limited(a0 + existing as u32, &source, room)?;
                (existing + source.len()) as u32
            }
            "OEMStrLen" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32,
            "OEMStrSize" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32 + 1,
            "swapl" => a0.swap_bytes(),
            "swaps" => (a0 as u16).swap_bytes() as u32,

            // --- Memória, versão e depuração ------------------------------------------
            "sysfree" => {
                self.heap.free(a0);
                SUCCESS
            }
            "err_strdup" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                bytes.push(0);
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr == 0 {
                    ENOMEMORY
                } else {
                    self.cpu.write_mem(ptr, &bytes)?;
                    self.cpu.write_u32(a1, ptr)?;
                    SUCCESS
                }
            }
            // `int err_realloc(uint32 uSize, void **pp)` — o `realloc` que devolve código de
            // erro e só troca o ponteiro se der certo.
            "err_realloc" => {
                let current = self.cpu.read_u32(a1)?;
                let ptr = self.realloc(current, a0)?;
                if ptr == 0 && a0 != 0 {
                    ENOMEMORY
                } else {
                    self.cpu.write_u32(a1, ptr)?;
                    SUCCESS
                }
            }
            // `uint32 GetAEEVersion(byte *pszFormatted, int nSize, uint16 wFlags)`: byte alto
            // da palavra alta é a versão maior, e assim por diante — daí `4.0.2.0`.
            "GetAEEVersion" => {
                if a0 != 0 {
                    if a2 & GAV_LATIN1 != 0 {
                        self.write_cstring_limited(a0, AEE_VERSION_TEXT, a1 as usize)?;
                    } else {
                        self.write_aechar(a0, AEE_VERSION_TEXT, a1 as usize / 2)?;
                    }
                }
                AEE_VERSION
            }
            // `uint32 GetFSFree(uint32 *pdwTotal)` — não temos cota de sistema de arquivos, e
            // responder um número grande é mais fiel que responder zero.
            "GetFSFree" => {
                if a0 != 0 {
                    self.cpu.write_u32(a0, FS_TOTAL)?;
                }
                FS_TOTAL
            }
            "aee_GetUTCSeconds" => self.elapsed_ms() / 1000,
            // `int32 aee_LocalTimeOffset(boolean *pbDaylightSavings)` — o emulador roda em UTC.
            "aee_LocalTimeOffset" => {
                if a0 != 0 {
                    self.cpu.write_mem(a0, &[0])?;
                }
                0
            }
            // Ganchos de depuração do BREW: existem para o log da plataforma, e o
            // comportamento correto sem ele é não fazer nada.
            "dumpheap" | "dbgevent" => SUCCESS,
            "dbgheapmark" => a0,
            // `int lockmem/unlockmem(void **ppHandle)` — o BREW do aparelho pode mover blocos
            // e por isso trava; a nossa heap é fixa, então travar é sempre um sucesso.
            "lockmem" | "unlockmem" => TRUE,
            // `char *aee_basename(const char *cpszPath)`
            "aee_basename" => {
                let path = self.cpu.read_cbytes(a0, MAX_STRING);
                let at = path
                    .iter()
                    .rposition(|&b| b == b'/' || b == b'\\')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                a0 + at as u32
            }
            "atoi" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                text.trim().parse::<i32>().unwrap_or(0) as u32
            }
            // A família de ponto flutuante da stdlib. Na AAPCS um `double` ocupa um par de
            // registradores com a palavra baixa primeiro, então `v1` vem em `r0:r1`, `v2` em
            // `r2:r3` e o que sobra vai para a pilha.
            "f_op" | "f_cmp" => {
                let (v1, v2) = (
                    fmath::from_words(a0, a1),
                    fmath::from_words(a2, self.cpu.read_reg(Reg::R3)),
                );
                let kind = self.cpu.read_u32(self.cpu.read_reg(Reg::Sp))?;
                if name == "f_cmp" {
                    let Some(answer) = fmath::cmp(v1, v2, kind) else {
                        return Ok(None);
                    };
                    u32::from(answer)
                } else {
                    let Some(value) = fmath::op(v1, v2, kind) else {
                        return Ok(None);
                    };
                    self.return_double(value)
                }
            }
            "f_calc" => {
                let Some(value) = fmath::calc(fmath::from_words(a0, a1), a2) else {
                    return Ok(None);
                };
                self.return_double(value)
            }
            "f_get" => {
                let Some(value) = fmath::get(a0) else {
                    return Ok(None);
                };
                self.return_double(value)
            }
            "f_assignint" => self.return_double(a0 as i32 as f64),
            "f_assignstr" | "strtod" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                let (value, consumed) = parse_leading_double(&text);
                // O `strtod` ainda devolve, pelo `char **`, onde parou de ler.
                if name == "strtod" && a1 != 0 {
                    self.cpu.write_u32(a1, a0 + consumed as u32)?;
                }
                self.return_double(value)
            }
            // `f_toint` arredonda para zero, como o cast do C; `trunc`/`utrunc` são a mesma
            // conta com o sinal do resultado mudando.
            "f_toint" | "trunc" => fmath::from_words(a0, a1).trunc() as i32 as u32,
            "utrunc" => fmath::from_words(a0, a1).trunc() as u32,
            // int vsnprintf(char *dst, int nSize, const char *fmt, va_list args) e
            // int vsprintf(char *dst, const char *fmt, va_list args).
            //
            // Na AAPCS o `va_list` é um ponteiro para a área de argumentos, então basta ler
            // palavras em sequência a partir dele.
            "vsnprintf" | "vsprintf" => {
                let (fmt_addr, args_addr, limit) = if name == "vsnprintf" {
                    (a2, self.cpu.read_reg(Reg::R3), a1 as usize)
                } else {
                    (a1, a2, usize::MAX)
                };
                let fmt = self.cpu.read_cstring(fmt_addr, MAX_STRING);
                // `AEEOldVaList` é `int **` no ARM (`inc/AEEOldVaList.h`): o que chega é o
                // endereço da variável `va_list`, não a área de argumentos. Sem essa
                // indireção o primeiro `%d` imprime o próprio ponteiro da pilha — foi o que
                // transformou `inva1_shotgun` em `inva537919380_shotgun` no Quake.
                let mut source = GuestArgs {
                    words: Vec::new(),
                    index: 0,
                    stack: self.cpu.read_u32(args_addr)?,
                    cpu: &self.cpu,
                };
                let text = cformat::format(&fmt, &mut source);
                self.write_cstring_limited(a0, &text, limit)?;
                text.len() as u32
            }
            // int snprintf(char *dst, int nSize, const char *fmt, ...)
            "snprintf" => {
                let fmt = self.cpu.read_cstring(a2, MAX_STRING);
                let text = self.format_from(3, &fmt);
                self.write_cstring_limited(a0, &text, a1 as usize)?;
                text.len() as u32
            }
            // `sprintf(char *dst, const char *fmt, ...)`: os variádicos começam em `r2`.
            "sprintf" => {
                let fmt = self.cpu.read_cstring(a1, MAX_STRING);
                let text = self.format_from(2, &fmt);
                self.write_cstring(a0, &text)?;
                text.len() as u32
            }
            "dbgprintf" => {
                let fmt = self.cpu.read_cstring(a0, MAX_STRING);
                let text = self.format_from(1, &fmt);
                self.record_debug(text);
                SUCCESS
            }
            // `IApplet *GetAppInstance(void)` — sem argumentos, o que explica por que o site
            // de chamada podia usar `r0` como rascunho para o ponteiro da função.
            //
            // O jogo chama isto *durante* a própria criação, depois que `AEEApplet_New` já
            // gravou o ponteiro no `void **ppObj` que passamos. Então, enquanto não terminamos
            // de criar o applet, a resposta certa é justamente o conteúdo desse ponteiro.
            "GetAppInstance" => match self.current_applet {
                0 => self.cpu.read_u32(self.module.out_module + 4)?,
                applet => applet,
            },
            // `GETUPTIMEMS` é desde que o aparelho ligou; `GETTIMESECONDS` é o **calendário**,
            // segundos desde 6 de janeiro de 1980. Responder o tempo ligado nos dois era dizer
            // que hoje é o dia da estreia do console: o Z-Wheel calcula o alarme do "próximo
            // dia" a partir daí e ficava girando — quinze milhões de voltas em dois métodos.
            "aee_GetTimeMS" | "aee_GetUpTimeMS" => self.elapsed_ms(),
            "aee_GetSeconds" => self.brew_seconds(),
            // void GETJULIANDATE(uint32 dwSecs, JulianType *pDate)
            //
            // `dwSecs` é o relógio do BREW: segundos desde 6 de janeiro de 1980, GMT. Zero quer
            // dizer "agora", e o nosso agora é o relógio virtual — o mesmo que responde ao
            // `GetSeconds`, para que as duas contas nunca se contradigam.
            "aee_GetJulianDate" => {
                // Helper não tem `this`: o primeiro argumento é o `r0`.
                let segundos = match a0 {
                    0 => self.brew_seconds(),
                    dado => dado,
                };
                if a1 != 0 {
                    let data = julian_date(segundos);
                    let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
                    self.cpu.write_mem(a1, &bytes)?;
                }
                SUCCESS
            }
            // Gerador simples e determinístico: repetir a mesma sessão tem que dar o mesmo
            // resultado, senão depurar jogo com aleatoriedade vira loteria.
            "aee_GetRand" => {
                let count = a1;
                for i in 0..count {
                    self.random_state = self
                        .random_state
                        .wrapping_mul(1_103_515_245)
                        .wrapping_add(12_345);
                    self.cpu
                        .write_mem(a0 + i, &[(self.random_state >> 16) as u8])?;
                }
                SUCCESS
            }
            "GetRAMFree" => (loader::HEAP_SIZE as u32).saturating_sub(self.heap.used()),
            // Dormir de verdade só atrasaria o emulador: o tempo do guest anda pelo relógio do
            // host de qualquer jeito.
            // void sleep(uint32 msecs) — o `MSLEEP` do SDK.
            //
            // No console o ARM realmente para e o tempo passa sozinho. Aqui o relógio anda com
            // as instruções, então quem dorme sem que o relógio avance dorme para sempre: o
            // Magical Drop 3 chamava isto cem milhões de vezes num laço que nunca vencia.
            // Adiantar o relógio é o que o aparelho faz.
            "sleep" => {
                self.clock_us += u64::from(a0.min(MAX_SLEEP_MS)) * 1000;
                SUCCESS
            }
            "getlasterror" => self.file_error,
            "aee_IsBadPtr" => u32::from(self.cpu.read_u32(a0).is_err()),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Prepara o retorno de um `double`: a palavra alta vai para `r1`, e a baixa é o valor
    /// que o despacho grava em `r0`.
    pub(super) fn return_double(&mut self, value: f64) -> u32 {
        let (low, high) = fmath::to_words(value);
        self.cpu.write_reg(Reg::R1, high);
        low
    }

    /// Formata uma string do guest tomando os variádicos a partir do registrador `first`.
    pub(super) fn format_from(&self, first: usize, fmt: &str) -> String {
        let words = [
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        ];
        let mut source = GuestArgs {
            words: words[first..].to_vec(),
            index: 0,
            stack: self.cpu.read_reg(Reg::Sp),
            cpu: &self.cpu,
        };
        cformat::format(fmt, &mut source)
    }

    pub(super) fn read_bytes(&self, addr: u32, len: u32) -> Result<Vec<u8>, CpuError> {
        let mut buf = vec![0u8; len as usize];
        if len > 0 {
            self.cpu.read_mem(addr, &mut buf)?;
        }
        Ok(buf)
    }

    /// Copia dentro da memória do guest. Passa pelo host, então regiões sobrepostas ficam
    /// corretas — que é justamente o que `memmove` promete.
    pub(super) fn copy_guest(&mut self, dst: u32, src: u32, len: u32) -> Result<(), CpuError> {
        let bytes = self.read_bytes(src, len)?;
        if len > 0 {
            self.cpu.write_mem(dst, &bytes)?;
        }
        Ok(())
    }

    /// Escreve uma string `AECHAR` (UTF-16 little-endian, terminada em zero).
    ///
    /// `max_units` limita quantos `AECHAR` cabem no destino, terminador incluído.
    pub(super) fn write_aechar(
        &mut self,
        addr: u32,
        text: &str,
        max_units: usize,
    ) -> Result<(), CpuError> {
        if addr == 0 || max_units == 0 {
            return Ok(());
        }
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.truncate(max_units.saturating_sub(1));
        units.push(0);
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Escreve uma string C respeitando o tamanho do destino.
    pub(super) fn write_cstring_limited(
        &mut self,
        addr: u32,
        text: &str,
        max_bytes: usize,
    ) -> Result<(), CpuError> {
        if addr == 0 || max_bytes == 0 {
            return Ok(());
        }
        // ISO-8859-1, como na leitura: um caractere, um byte. Além de manter o acento, é o
        // que faz o corte por tamanho cair sempre em fronteira de caractere.
        let mut bytes = crate::cpu::latin1_encode(text);
        bytes.truncate(max_bytes.saturating_sub(1));
        bytes.push(0);
        self.cpu.write_mem(addr, &bytes)
    }

    pub(super) fn write_cstring(&mut self, addr: u32, text: &str) -> Result<(), CpuError> {
        let mut bytes = crate::cpu::latin1_encode(text);
        bytes.push(0);
        self.cpu.write_mem(addr, &bytes).map_err(|e| {
            CpuError(format!(
                "{e} ao escrever {} bytes em {addr:#010x}",
                bytes.len()
            ))
        })
    }
}
