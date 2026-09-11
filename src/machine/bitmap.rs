//! IBitmap e o DIB: as superfícies, o blit e a sincronia com a memória do jogo.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Desenha uma imagem numa superfície do jogo chamando o `BltIn` dela.
    ///
    /// É a mesma coreografia do BREW: montamos um `IBitmap` nosso com a imagem decodificada e
    /// passamos como origem. O `BltIn` do jogo então pede `QueryInterface(AEECLSID_DIB)` no
    /// nosso bitmap e lê os pixels pelos campos públicos, que é justamente o que o `IDIB`
    /// existe para oferecer. Conferido na desmontagem: o `BltIn` do Bejeweled Twist faz esse
    /// `QueryInterface` com `0x01001045` e depois lê `cx`, `cy` e `nColorScheme`.
    pub(super) fn blit_into_foreign(&mut self, blit: PendingBlit, budget: u64) -> Result<(), CpuError> {
        let Some(info) = self.images.get(&blit.image).cloned() else {
            return Ok(());
        };
        let (frame_width, offset) = match (blit.frame, info.frame_width) {
            (Some(n), width) if width > 0 => (width as u32, n * width as u32),
            _ => (info.width, 0),
        };

        // A origem é uma superfície nossa, criada só para esta chamada.
        let source = self.new_object(Interface::Bitmap)?;
        if source == 0 {
            return Ok(());
        }
        let mut surface = Framebuffer::new(frame_width, info.height);
        for row in 0..info.height {
            for column in 0..frame_width {
                let index = (row * info.width + column + offset) as usize;
                let opaque = info.opaque.get(index).copied().unwrap_or(true);
                let pixel = match (opaque, info.pixels.get(index)) {
                    (true, Some(&pixel)) => pixel,
                    // Sem canal alfa no destino, o transparente vira uma cor reservada — é
                    // como o BREW resolve, e o `BltIn` respeita a `ncTransparent` do `IDIB`.
                    _ => TRANSPARENT_KEY,
                };
                surface.set_pixel_native(column as i32, row as i32, pixel);
            }
        }
        self.bitmaps.insert(source, surface);
        self.transparency.insert(source, TRANSPARENT_KEY);
        self.expose_dib(source)?;

        let vtable = self.cpu.read_u32(blit.target)?;
        let blt_in = self.cpu.read_u32(vtable + BITMAP_BLT_IN_SLOT * 4)?;
        // BltIn(po, xDst, yDst, dx, dy, pSrc, xSrc, ySrc, rop)
        let outcome = self.call_guest_with_stack(
            blt_in,
            [blit.target, blit.x as u32, blit.y as u32, frame_width],
            &[info.height, source, 0, 0, AEE_RO_TRANSPARENT],
            budget,
        )?;
        if !matches!(outcome, Outcome::Returned { code: 0 }) {
            self.assumptions
                .insert("o BltIn de uma superfície do jogo recusou o desenho");
        }

        self.objects.release(source);
        self.bitmaps.remove(&source);
        self.dib_buffers.remove(&source);
        self.transparency.remove(&source);
        Ok(())
    }

    /// Descobre onde ficam os pixels de um `IBitmap` implementado pelo próprio jogo.
    ///
    /// É o mesmo caminho que o BREW usa: `QueryInterface(AEECLSID_DIB)` no objeto, e o `IDIB`
    /// que volta traz `pBmp`, `cx`, `cy` e `nPitch` como campos públicos. Com isso a superfície
    /// do jogo entra no mesmo mecanismo de sincronização das nossas — desenhamos no host e o
    /// resultado é copiado para a memória dele.
    pub(super) fn probe_foreign_surface(&mut self, target: u32, budget: u64) -> Result<(), CpuError> {
        let Ok(vtable) = self.cpu.read_u32(target) else {
            return Ok(());
        };
        let Ok(query) = self.cpu.read_u32(vtable + BITMAP_QUERY_INTERFACE_SLOT * 4) else {
            return Ok(());
        };
        // O ponteiro de saída precisa viver na memória do guest.
        let Some(out) = self.heap.alloc(4) else {
            return Ok(());
        };
        // O `IDIB` tem dois IIDs: o atual e o que o BREW 2.0 usava. Um bitmap escrito para a
        // plataforma antiga só reconhece o segundo.
        let mut dib = 0;
        let mut outcome = Outcome::Returned { code: EFAILED };
        for iid in [AEECLSID_DIB, AEEIID_DIB_20] {
            self.cpu.write_u32(out, 0)?;
            outcome = self.call_guest(query, [target, iid, out, 0], budget)?;
            dib = self.cpu.read_u32(out).unwrap_or(0);
            if matches!(outcome, Outcome::Returned { code: 0 }) && dib != 0 {
                break;
            }
        }
        self.heap.free(out);

        let failed = !matches!(outcome, Outcome::Returned { code: 0 });
        if failed || dib == 0 {
            // O Bejeweled Twist é assim: o `QueryInterface` da superfície dele é literalmente
            // `mov r0, #0x14; bx lr` — devolve `ECLASSNOTSUPPORT` sempre. Nessa superfície o
            // único método de desenho implementado de verdade é o `BltIn`.
            self.assumptions
                .insert("uma superfície do jogo não expõe IDIB; o desenho nela ainda se perde");
            return Ok(());
        }

        let buffer = self.cpu.read_u32(dib + 8)?;
        let mut fields = [0u8; 10];
        self.cpu.read_mem(dib + 20, &mut fields)?;
        let cx = u16::from_le_bytes([fields[0], fields[1]]) as u32;
        let cy = u16::from_le_bytes([fields[2], fields[3]]) as u32;
        let pitch = i16::from_le_bytes([fields[4], fields[5]]) as i32;
        let depth = fields[8];

        // Só sabemos tratar o formato da tela do console: RGB565, linhas contíguas e para
        // baixo. Qualquer outra coisa é melhor recusar do que desenhar torto.
        if cx == 0
            || cy == 0
            || buffer == 0
            || depth != COLOR_DEPTH as u8
            || pitch != (cx * 2) as i32
        {
            self.assumptions
                .insert("uma superfície do jogo usa um formato que ainda não sabemos desenhar");
            return Ok(());
        }

        self.bitmaps.insert(target, Framebuffer::new(cx, cy));
        self.dib_buffers.insert(target, buffer);
        self.sync_from_guest(target)?;
        Ok(())
    }

    /// O bitmap da tela, criado na primeira vez que alguém pede.
    pub(super) fn device_bitmap(&mut self) -> Result<u32, CpuError> {
        if self.device_bitmap != 0 {
            return Ok(self.device_bitmap);
        }
        let addr = self.new_object(Interface::Bitmap)?;
        if addr != 0 {
            let screen = std::mem::replace(&mut self.screen, Framebuffer::new(1, 1));
            self.bitmaps.insert(addr, screen);
            self.device_bitmap = addr;
            self.display_target = addr;
            // **Uma referência nossa, que nunca é solta.** Quem pede o bitmap da tela solta o
            // que recebeu, como manda a convenção; mas o dono dele é o display, não quem
            // pediu. Sem esta contagem o endereço voltava para a lista de livres e o
            // `CreateCompatibleBitmap` seguinte gravava a superfície dele por cima da tela: a
            // Z-Wheel entrou no Stage, criou uma superfície de 214×34, e era ela que aparecia
            // na janela no lugar dos 640×480.
            self.objects.add_ref(addr);
        }
        Ok(addr)
    }

    /// Métodos de `IBitmap`, despachados pelo nome do slot.
    pub(super) fn bitmap_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Bitmap.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            // `IBitmap` tem `QueryInterface`, então AddRef e Release precisam ser tratados aqui:
            // o braço genérico do despacho só alcança interfaces que não têm tratamento próprio.
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // int QueryInterface(IBitmap *, AEECLSID, void **) — o jogo usa isto para pedir um
            // `IDIB`, que dá acesso direto aos pixels. Ainda não oferecemos essa interface, e
            // `ECLASSNOTSUPPORT` é a resposta correta para isso: o BREW espera que o app tenha
            // caminho alternativo quando a plataforma não expõe o DIB.
            "QueryInterface" => {
                let requested = self.cpu.read_reg(Reg::R1);
                let out = self.cpu.read_reg(Reg::R2);
                let answer = match requested {
                    AEEIID_IBITMAP => Some(this),
                    // Um `IDIB` *é* um `IBitmap` — a struct começa com a vtable de `IBitmap` e
                    // só acrescenta campos públicos. Então o próprio objeto serve, desde que
                    // os campos estejam preenchidos.
                    AEECLSID_DIB | AEEIID_DIB_20 | AEEIID_DIB_ANTIGO => {
                        self.expose_dib(this)?;
                        Some(this)
                    }
                    _ => None,
                };
                match answer {
                    Some(pointer) => {
                        if out != 0 {
                            self.cpu.write_u32(out, pointer)?;
                        }
                        self.objects.add_ref(this);
                        SUCCESS
                    }
                    None => {
                        if out != 0 {
                            self.cpu.write_u32(out, 0)?;
                        }
                        self.unknown_classes.insert(requested);
                        ECLASSNOTSUPPORT
                    }
                }
            }
            // NativeColor no nosso caso é o próprio RGB565 do framebuffer.
            "RGBToNative" => Rgb::from_rgbval(self.cpu.read_reg(Reg::R1)).to_rgb565() as u32,
            "NativeToRGB" => {
                let native = self.cpu.read_reg(Reg::R1) as u16;
                to_rgbval(Rgb::from_rgb565(native))
            }
            // int DrawPixel(IBitmap *po, unsigned x, unsigned y, NativeColor c, AEERasterOp rop)
            "DrawPixel" => {
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let color = self.cpu.read_reg(Reg::R3) as u16;
                if let Some(fb) = self.bitmaps.get_mut(&this) {
                    fb.set_pixel_native(x, y, color);
                }
                // O buffer do jogo é a fonte da verdade quando ele existe, então o pixel vai
                // para os dois lugares — e só ele, não a superfície inteira.
                if let Some(at) = self.dib_pixel(this, x, y) {
                    self.cpu.write_mem(at, &color.to_le_bytes())?;
                }
                SUCCESS
            }
            "GetPixel" => {
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                // Do buffer do jogo, quando há um: ele pode ter escrito ali direto.
                let value = match self.dib_pixel(this, x, y) {
                    Some(at) => {
                        let mut bytes = [0u8; 2];
                        self.cpu.read_mem(at, &mut bytes)?;
                        u16::from_le_bytes(bytes)
                    }
                    None => self
                        .bitmaps
                        .get(&this)
                        .map(|fb| fb.get_pixel(x, y))
                        .unwrap_or(0),
                };
                let out = self.cpu.read_reg(Reg::R3);
                if out != 0 {
                    self.cpu.write_u32(out, value as u32)?;
                }
                SUCCESS
            }
            // int DrawHScanline(IBitmap *po, unsigned y, unsigned xMin, unsigned xMax, ...)
            "DrawHScanline" => {
                let y = self.cpu.read_reg(Reg::R1) as i32;
                let x_min = self.cpu.read_reg(Reg::R2) as i32;
                let x_max = self.cpu.read_reg(Reg::R3) as i32;
                let color = self.stack_arg(0)? as u16;
                if let Some(fb) = self.bitmaps.get_mut(&this) {
                    for x in x_min..=x_max {
                        fb.set_pixel_native(x, y, color);
                    }
                }
                SUCCESS
            }
            // int FillRect(IBitmap *po, const AEERect *prc, NativeColor color, AEERasterOp rop)
            // int FillRect(IBitmap *po, const AEERect *prc, NativeColor color, AEERasterOp rop)
            //
            // `IBITMAP_FillRect.htm`: só `AEE_RO_COPY` e `AEE_RO_XOR` valem; qualquer outra
            // operação é `EUNSUPPORTED` e **não desenha**. Enquanto ignorávamos o `rop`, o
            // Bejeweled Twist pintava a tela inteira de preto uma vez por quadro com um
            // `AEE_RO_TRANSPARENT` que o console teria recusado.
            "FillRect" => {
                let rop = self.cpu.read_reg(Reg::R3);
                let rect = self.read_rect(self.cpu.read_reg(Reg::R1))?;
                let color = self.cpu.read_reg(Reg::R2) as u16;
                // Com `AEE_RO_TRANSPARENT`, preencher com a própria cor transparente não
                // escreve nada. Ignorar o `rop` custava caro: o Bejeweled Twist chama exatamente
                // assim, com cor zero, e a tela inteira era apagada uma vez por quadro.
                let transparent = self.transparency.get(&this).copied().unwrap_or(0);
                if rop == AEE_RO_TRANSPARENT && color == transparent {
                    return Ok(Some(SUCCESS));
                }
                if let (Some(rect), Some(fb)) = (rect, self.bitmaps.get_mut(&this)) {
                    // `AEE_RO_COPY` é o normal; qualquer outra operação que chegue aqui ainda
                    // pinta, porque recusar o desenho é pior do que pintar demais.
                    if rop == AEE_RO_XOR {
                        fb.xor_rect_native(rect, color);
                    } else {
                        fb.fill_rect_native(rect, color);
                    }
                }
                SUCCESS
            }
            // int BltIn(IBitmap *po, int xDst, int yDst, int dx, int dy,
            //           IBitmap *pSrc, int xSrc, int ySrc, AEERasterOp rop)
            "BltIn" => {
                let dst = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let src = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                self.blit(this, dst, size, src, origin, rop);
                SUCCESS
            }
            // BltOut inverte os papéis: a fonte é `po` e o destino vem no argumento.
            "BltOut" => {
                let dst_pos = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let dst = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                self.blit(dst, dst_pos, size, this, origin, rop);
                SUCCESS
            }
            // int GetInfo(IBitmap *po, AEEBitmapInfo *pinfo, int nSize)
            "GetInfo" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    let (cx, cy) = self
                        .bitmaps
                        .get(&this)
                        .map(|fb| (fb.width(), fb.height()))
                        .unwrap_or((0, 0));
                    self.cpu.write_u32(out, cx)?;
                    self.cpu.write_u32(out + 4, cy)?;
                    self.cpu.write_u32(out + 8, COLOR_DEPTH as u32)?;
                }
                SUCCESS
            }
            // int CreateCompatibleBitmap(IBitmap *po, IBitmap **ppIBitmap, uint16 w, uint16 h)
            "CreateCompatibleBitmap" => {
                let out = self.cpu.read_reg(Reg::R1);
                let width = self.cpu.read_reg(Reg::R2) & 0xffff;
                let height = self.cpu.read_reg(Reg::R3) & 0xffff;
                let addr = self.new_object(Interface::Bitmap)?;
                if addr == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.bitmaps.insert(addr, Framebuffer::new(width, height));
                if out != 0 {
                    self.cpu.write_u32(out, addr)?;
                }
                SUCCESS
            }
            "SetTransparencyColor" => {
                self.transparency
                    .insert(this, self.cpu.read_reg(Reg::R1) as u16);
                SUCCESS
            }
            "GetTransparencyColor" => {
                let value = self.transparency.get(&this).copied().unwrap_or(0);
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    self.cpu.write_u32(out, value as u32)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Prepara um bitmap para ser usado como `IDIB`: reserva o buffer de pixels na memória do
    /// guest, copia o conteúdo atual para lá e preenche os campos públicos da struct.
    ///
    /// Layout, de `inc/AEEIDIB.h`: `pvt`, `pPaletteMap`, `pBmp`, `pRGB`, `ncTransparent`,
    /// `cx`, `cy`, `nPitch`, `cntRGB`, `nDepth`, `nColorScheme` e seis bytes reservados.
    pub(super) fn expose_dib(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let (cx, cy) = (fb.width(), fb.height());
        let precisa = cx * 2 * cy;
        // **O cabeçalho é reescrito toda vez, e não só na primeira.** O endereço de um objeto
        // volta a ser usado quando o anterior é liberado, e o bitmap novo tem outro tamanho:
        // com a checagem por endereço, o `IDIB` de uma imagem nova continuava anunciando o
        // tamanho da imagem anterior.
        //
        // Foi o que quebrou o texto do Tekken 2. Ele decodifica nove imagens em sequência,
        // liberando cada uma antes da seguinte — todas nasceram no mesmo endereço. O `IDIB`
        // dizia 200x112 para todas, então o jogo criava uma página de 200x112 para uma folha de
        // letras de 360x280, copiava só o canto dela e depois pedia cada glifo por coordenada
        // da folha inteira. O que caía fora virava bloco: o menu inteiro saía com as palavras
        // como retângulos laranja.
        //
        // O buffer é reaproveitado quando cabe. Reservar outro a cada exposição também acerta o
        // tamanho, mas a região de superfícies **não recicla**: um jogo que decodifique centenas
        // de imagens a esgotaria.
        let cabe = self
            .dib_capacity
            .get(&bitmap)
            .is_some_and(|&tinha| tinha >= precisa);
        if !cabe {
            let Some(buffer) = self.surface_alloc(precisa) else {
                return Ok(());
            };
            self.dib_buffers.insert(bitmap, buffer);
            self.dib_capacity.insert(bitmap, precisa);
            // A primeira exposição precisa publicar os pixels atuais. Exposições seguintes
            // apenas atualizam o cabeçalho: reescrever a superfície inteira em cada
            // QueryInterface apagava alterações feitas pelo guest e custava dezenas de ms em
            // jogos que consultam o bitmap a cada quadro.
            self.sync_to_guest(bitmap)?;
        }
        self.write_dib_header(bitmap)
    }

    /// Escreve os campos públicos do `IDIB` de um bitmap: tamanho, passo, profundidade e o
    /// ponteiro para os pixels — este último só quando eles já existem.
    ///
    /// Um `IBitmap` de software do BREW **é** um `IDIB`: a struct começa com a vtable de
    /// `IBitmap` e segue com campos públicos, e o jogo lê esses campos direto, sem pedir nada.
    /// O Peggle é o caso: ele decodifica o PNG, pergunta o tamanho pelos campos e imprime
    /// `-size 0/0` no log dele quando não acha. Aí monta cada sprite como um quadrado de lado
    /// zero — 76.618 dos 77.208 triângulos de um quadro saíam degenerados, e a tela ficava
    /// preta com o jogo desenhando o tempo todo.
    ///
    /// Fora do decodificador, os pixels continuam sendo alocados só no `QueryInterface`, que é
    /// quando o jogo declara que vai mexer neles: a região de superfícies não recicla, e toda
    /// superfície publicada entra no laço que sincroniza os pixels a cada chamada que os toca.
    /// Escrever o cabeçalho sem os pixels seria pior que não escrever nada — o jogo passa a
    /// confiar no `pBmp` e desreferencia o zero.
    pub(super) fn write_dib_header(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let (cx, cy) = (fb.width(), fb.height());
        let pitch = cx * 2;
        let buffer = self.dib_buffers.get(&bitmap).copied().unwrap_or(0);
        let transparent = self.transparency.get(&bitmap).copied().unwrap_or(0) as u32;
        self.cpu.write_u32(bitmap + 4, 0)?; // pPaletteMap
        self.cpu.write_u32(bitmap + 8, buffer)?; // pBmp
        self.cpu.write_u32(bitmap + 12, 0)?; // pRGB: RGB565 não tem paleta
        self.cpu.write_u32(bitmap + 16, transparent)?;
        self.cpu
            .write_mem(bitmap + 20, &(cx as u16).to_le_bytes())?;
        self.cpu
            .write_mem(bitmap + 22, &(cy as u16).to_le_bytes())?;
        self.cpu
            .write_mem(bitmap + 24, &(pitch as i16).to_le_bytes())?;
        self.cpu.write_mem(bitmap + 26, &0u16.to_le_bytes())?; // cntRGB
        // `nDepth` em bits e `nColorScheme` com o código de `AEEIDIB.h`. Sem o esquema correto o
        // jogo não sabe como interpretar os pixels e desiste.
        self.cpu
            .write_mem(bitmap + 28, &[COLOR_DEPTH as u8, IDIB_COLORSCHEME_565])?;
        self.cpu.write_mem(bitmap + 30, &[0u8; 6])?;
        Ok(())
    }

    /// Reserva espaço na região de superfícies.
    pub(super) fn surface_alloc(&mut self, bytes: u32) -> Option<u32> {
        let addr = self.surface_next;
        let end = loader::SURFACE_BASE + loader::SURFACE_SIZE as u32;
        if addr.checked_add(bytes)? > end {
            return None;
        }
        // Alinha em 4 bytes para que o jogo possa escrever palavras inteiras.
        self.surface_next = (addr + bytes).div_ceil(4) * 4;
        Some(addr)
    }

    /// Copia os pixels do host para o buffer que o jogo enxerga.
    pub(super) fn sync_to_guest(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(&buffer) = self.dib_buffers.get(&bitmap) else {
            return Ok(());
        };
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let bytes = fb.to_rgb565_bytes();
        self.cpu.write_mem(buffer, &bytes)
    }

    /// Traz de volta o que o jogo escreveu direto no buffer.
    /// O endereço de um pixel dentro do buffer que o jogo enxerga, quando ele existe.
    ///
    /// É o que permite `DrawPixel` e `GetPixel` mexerem em dois bytes em vez de mandarem a
    /// superfície inteira de um lado para o outro.
    pub(super) fn dib_pixel(&self, bitmap: u32, x: i32, y: i32) -> Option<u32> {
        let &buffer = self.dib_buffers.get(&bitmap)?;
        let fb = self.bitmaps.get(&bitmap)?;
        let (width, height) = (fb.width() as i32, fb.height() as i32);
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        Some(buffer + ((y * width + x) as u32) * 2)
    }

    pub(super) fn sync_from_guest(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(&buffer) = self.dib_buffers.get(&bitmap) else {
            return Ok(());
        };
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let mut bytes = vec![0u8; (fb.width() * fb.height() * 2) as usize];
        self.cpu.read_mem(buffer, &mut bytes)?;
        if let Some(fb) = self.bitmaps.get_mut(&bitmap) {
            fb.load_rgb565_bytes(&bytes);
        }
        Ok(())
    }

    /// Sincroniza todas as superfícies que o jogo pode ter alterado direto.
    ///
    /// Chamado só nas interfaces que mexem em pixels: um jogo faz dezenas de milhares de
    /// chamadas de outras APIs, e copiar 600 KB em cada uma seria inviável.
    pub(super) fn sync_surfaces_in(&mut self) -> Result<(), CpuError> {
        for bitmap in self.dib_buffers.keys().copied().collect::<Vec<_>>() {
            self.sync_from_guest(bitmap)?;
        }
        Ok(())
    }

    pub(super) fn sync_surfaces_out(&mut self) -> Result<(), CpuError> {
        for bitmap in self.dib_buffers.keys().copied().collect::<Vec<_>>() {
            self.sync_to_guest(bitmap)?;
        }
        Ok(())
    }

    /// Copia de uma superfície para outra, respeitando a cor transparente quando o raster op
    /// pede. Precisa tirar o destino do mapa antes para não ter duas referências mutáveis.
    pub(super) fn blit(
        &mut self,
        dst: u32,
        dst_pos: (i32, i32),
        size: (i32, i32),
        src: u32,
        src_pos: (i32, i32),
        rop: u32,
    ) {
        if dst == src {
            return;
        }
        let Some(mut target) = self.bitmaps.remove(&dst) else {
            return;
        };
        if let Some(source) = self.bitmaps.get(&src) {
            let transparent = if rop == AEE_RO_TRANSPARENT {
                self.transparency.get(&src).copied()
            } else {
                None
            };
            target.blit(
                dst_pos.0,
                dst_pos.1,
                size.0,
                size.1,
                source,
                src_pos.0,
                src_pos.1,
                transparent,
            );
        }
        self.bitmaps.insert(dst, target);
    }

    /// Lê um `AEERect` da memória do guest. `None` quando o ponteiro é nulo.
    pub(super) fn clip_rect(&self, rect: Rect) -> Option<Rect> {
        clip_rect(self.clip, rect)
    }

    pub(super) fn clip_blit(&self, dst: (i32, i32), size: (i32, i32), src: (i32, i32)) -> Option<Blit> {
        clip_blit(self.clip, dst, size, src)
    }

    pub(super) fn read_rect(&self, addr: u32) -> Result<Option<Rect>, CpuError> {
        if addr == 0 {
            return Ok(None);
        }
        let mut bytes = [0u8; 8];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok(Some(Rect::from_bytes(bytes)))
    }

    /// Cada superfície viva como um BMP, para inspeção: endereço, tamanho e bytes.
    ///
    /// Existe porque "o jogo desenha e a tela fica preta" tem duas causas possíveis, e só o
    /// conteúdo das superfícies as separa: ou o jogo desenhou em algo que não vai para a tela,
    /// ou não desenhou. Descobrir isso por instrumentação temporária custou duas investigações.
    pub fn superficies(&self) -> Vec<(u32, u32, u32, Vec<u8>)> {
        self.bitmaps
            .iter()
            .map(|(&addr, fb)| (addr, fb.width(), fb.height(), fb.to_bmp()))
            .collect()
    }
}
