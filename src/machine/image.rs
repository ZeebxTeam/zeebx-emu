//! IImage e IImageDecoder: decodificar o que o jogo traz e desenhar na tela.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IImageDecoder` e o `IForceFeed` que o alimenta.
    ///
    /// O jogo cria o decodificador, pede a ele a interface de entrada, escreve o arquivo em
    /// pedaços, fecha com uma escrita vazia e busca o bitmap. É o caminho que o Heavy Weapon, o
    /// Tork and Kral e o Peggle usam para as imagens deles.
    pub(super) fn decoder_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        // Um `IForceFeed` trabalha sempre sobre o decodificador que o criou.
        let decoder = match iface {
            Interface::ForceFeed => self.feeds.get(&this).copied().unwrap_or(0),
            _ => this,
        };
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.feeds.remove(&this);
                    self.decoders.remove(&this);
                }
                remaining
            }
            (_, "QueryInterface") => {
                if a2 == 0 {
                    return Ok(Some(EBADPARM));
                }
                match a1 {
                    AEEIID_FORCEFEED => {
                        let feed = self.new_object(Interface::ForceFeed)?;
                        if feed == 0 {
                            return Ok(Some(ENOMEMORY));
                        }
                        self.feeds.insert(feed, decoder);
                        self.cpu.write_u32(a2, feed)?;
                        SUCCESS
                    }
                    _ => {
                        self.unknown_classes.insert(a1);
                        self.cpu.write_u32(a2, 0)?;
                        ECLASSNOTSUPPORT
                    }
                }
            }
            // int Write(IForceFeed *, void *pBuf, int cb)
            //
            // Escrita vazia é o fim do arquivo — é assim que o exemplo do SDK fecha a entrega.
            // Nada a fazer aqui: a decodificação acontece no `GetBitmap`, e adiantá-la só
            // gastaria trabalho se o jogo desistisse no meio.
            (Interface::ForceFeed, "Write") => {
                let count = a2 as usize;
                if a1 != 0 && count > 0 {
                    let mut bytes = vec![0u8; count];
                    self.cpu.read_mem(a1, &mut bytes)?;
                    let state = self.decoders.entry(decoder).or_default();
                    if state.fed.len() + count <= MAX_DECODED_INPUT {
                        state.fed.extend_from_slice(&bytes);
                    }
                }
                SUCCESS
            }
            (Interface::ForceFeed, "Reset") => {
                self.decoders.remove(&decoder);
                SUCCESS
            }
            // int GetBitmap(IImageDecoder *, IBitmap **ppiBitmap)
            (Interface::ImageDecoder, "GetBitmap") => {
                let bitmap = self.decoded_bitmap(decoder)?;
                if a1 != 0 {
                    self.cpu.write_u32(a1, bitmap)?;
                }
                match bitmap {
                    0 => EFAILED,
                    _ => SUCCESS,
                }
            }
            // int GetRop(IImageDecoder *) — com o que desenhar o bitmap devolvido.
            (Interface::ImageDecoder, "GetRop") => {
                self.decoded_bitmap(decoder)?;
                match self.decoders.get(&decoder).is_some_and(|d| d.transparent) {
                    true => AEE_RO_TRANSPARENT,
                    false => AEE_RO_COPY,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// O bitmap de um decodificador, decodificando na primeira vez que é pedido.
    pub(super) fn decoded_bitmap(&mut self, decoder: u32) -> Result<u32, CpuError> {
        if let Some(state) = self.decoders.get(&decoder) {
            if let Some(bitmap) = state.bitmap {
                return Ok(bitmap);
            }
        }
        let Some(fed) = self.decoders.get(&decoder).map(|d| d.fed.clone()) else {
            return Ok(0);
        };
        let Some(image) = decode_png(&fed) else {
            self.assumptions
                .insert("um decodificador recebeu dados que não são um PNG");
            return Ok(0);
        };
        let addr = self.bitmap_from_decoded(&image)?;
        if addr == 0 {
            return Ok(0);
        }
        let transparent = self.transparency.contains_key(&addr);
        if let Some(state) = self.decoders.get_mut(&decoder) {
            state.bitmap = Some(addr);
            state.transparent = transparent;
        }
        Ok(addr)
    }

    /// Um `IBitmap` com a imagem já decodificada dentro.
    ///
    /// Sai com os campos públicos do `IDIB` preenchidos, porque um `IBitmap` de software do
    /// BREW é um `IDIB` e o jogo lê esses campos sem pedir a interface.
    pub(super) fn bitmap_from_decoded(&mut self, image: &DecodedImage) -> Result<u32, CpuError> {
        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let mut surface = Framebuffer::new(image.width, image.height);
        let mut transparent = false;
        for row in 0..image.height {
            for column in 0..image.width {
                let index = (row * image.width + column) as usize;
                let opaque = image.opaque.get(index).copied().unwrap_or(true);
                transparent |= !opaque;
                // Sem canal alfa no destino, o transparente vira a cor reservada — é como o
                // BREW resolve, e é o que o `GetRop` anuncia ao jogo em seguida.
                let pixel = match (opaque, image.pixels.get(index)) {
                    (true, Some(&pixel)) => pixel,
                    _ => TRANSPARENT_KEY,
                };
                surface.set_pixel_native(column as i32, row as i32, pixel);
            }
        }
        self.bitmaps.insert(addr, surface);
        if transparent {
            self.transparency.insert(addr, TRANSPARENT_KEY);
        }
        self.expose_dib(addr)?;
        Ok(addr)
    }

    /// Decodifica a imagem em `buffer` e devolve um `IBitmap` com ela.
    ///
    /// O ponteiro devolvido vai direto para o `IDISPLAY_BitBlt`, que recebe um `IBitmap *` — e
    /// é por isso que o "formato nativo" aqui é um bitmap nosso, e não um bloco solto de
    /// pixels: assim o desenho segue pelo mesmo caminho de todo o resto.
    ///
    /// O tamanho do bloco não vem por parâmetro: quem diz quanto ler é o cabeçalho da própria
    /// imagem, e por ora só o BMP — que é o que os jogos passam — declara o seu.
    pub(super) fn setup_native_image(
        &mut self,
        buffer: u32,
        info: u32,
        realloc: u32,
    ) -> Result<u32, CpuError> {
        if realloc != 0 {
            // A imagem sai numa alocação nossa, e é isso que este sinalizador informa.
            self.cpu.write_mem(realloc, &[1])?;
        }
        let Some(len) = self.encoded_image_len(buffer)? else {
            return Ok(0);
        };
        let mut bytes = vec![0u8; len];
        self.cpu.read_mem(buffer, &mut bytes)?;
        let Ok(image) = crate::video::icon::decode(&bytes) else {
            self.assumptions
                .insert("uma imagem nativa veio num formato que não sabemos ler");
            return Ok(0);
        };

        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let (width, height) = (image.width as u32, image.height as u32);
        let mut fb = Framebuffer::new(width, height);
        for y in 0..image.height {
            for x in 0..image.width {
                let at = (y * image.width + x) * 4;
                let color = Rgb {
                    r: image.rgba[at],
                    g: image.rgba[at + 1],
                    b: image.rgba[at + 2],
                };
                fb.set_pixel_native(x as i32, y as i32, color.to_rgb565());
            }
        }
        self.bitmaps.insert(addr, fb);

        if info != 0 {
            let (cx, cy) = (width as u16, height as u16);
            self.cpu.write_mem(info, &cx.to_le_bytes())?;
            self.cpu.write_mem(info + 2, &cy.to_le_bytes())?;
            // `nColors` é zero para quem tem mais de 65535 cores, e `bAnimated` é falso.
            self.cpu.write_mem(info + 4, &[0u8; 4])?;
            self.cpu.write_mem(info + 8, &cx.to_le_bytes())?;
        }
        Ok(addr)
    }

    /// Quanto ler de um bloco de imagem, pelo cabeçalho dela.
    pub(super) fn encoded_image_len(&self, buffer: u32) -> Result<Option<usize>, CpuError> {
        if buffer == 0 {
            return Ok(None);
        }
        let mut header = [0u8; 6];
        self.cpu.read_mem(buffer, &mut header)?;
        // O BMP declara o tamanho do arquivo na palavra seguinte à assinatura.
        if &header[0..2] != b"BM" {
            return Ok(None);
        }
        let size = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
        Ok((size > 0 && size <= MAX_NATIVE_IMAGE).then_some(size))
    }

    /// `IImage` sobre o decodificador de PNG (`AEECLSID_PNG` = `0x01004004`), de
    /// `inc/AEEIImage.h`.
    ///
    /// O jogo alimenta a imagem com um `IMemAStream` (`SetStream`), lê as dimensões com
    /// `GetInfo` e desenha com `Draw`. A decodificação em si é PNG padrão — não há nada de
    /// proprietário aqui, ao contrário do que os bytes de alta entropia do `resources.dat`
    /// sugeriam à primeira vista.
    pub(super) fn image_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Image.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.images.remove(&this);
                    self.image_bitmaps.remove(&this);
                }
                remaining
            }
            // void SetStream(IImage *, IAStream *ps) — consome o stream inteiro de uma vez.
            "SetStream" => {
                self.decode_image(this, a1)?;
                SUCCESS
            }
            // void GetInfo(IImage *, AEEImageInfo *pi): cx, cy, nColors (uint16), bAnimated
            // (boolean) e cxFrame (uint16).
            "GetInfo" => {
                let (cx, cy, frame) = match self.images.get(&this) {
                    Some(image) => (image.width as u16, image.height as u16, image.frame_width),
                    None => (0, 0, 0),
                };
                if a1 != 0 {
                    self.cpu.write_mem(a1, &cx.to_le_bytes())?;
                    self.cpu.write_mem(a1 + 2, &cy.to_le_bytes())?;
                    // `nColors` é zero para imagens com mais de 65535 cores, que é o caso.
                    self.cpu.write_mem(a1 + 4, &0u16.to_le_bytes())?;
                    self.cpu.write_mem(a1 + 6, &[0u8, 0])?;
                    self.cpu.write_mem(a1 + 8, &frame.to_le_bytes())?;
                }
                SUCCESS
            }
            // void SetParm(IImage *, int nParm, int p1, int p2)
            "SetParm" => {
                self.image_set_parm(this, a1, a2, a3)?;
                SUCCESS
            }
            "Draw" => {
                self.draw_image(this, a1 as i32, a2 as i32, None)?;
                SUCCESS
            }
            // void DrawFrame(IImage *, int nFrame, int x, int y)
            "DrawFrame" => {
                self.draw_image(this, a2 as i32, a3 as i32, Some(a1))?;
                SUCCESS
            }
            // Sem animação, `Start` é um `Draw` e `Stop` não tem o que parar.
            "Start" => {
                self.draw_image(this, a1 as i32, a2 as i32, None)?;
                SUCCESS
            }
            "Stop" | "HandleEvent" => SUCCESS,
            // void Notify(IImage *, PFNIMAGEINFO pfn, void *pUser)
            //
            "Notify" => {
                if a1 != 0 {
                    self.image_notify.insert(
                        this,
                        Callback {
                            function: a1,
                            context: a2,
                        },
                    );
                    // Uma imagem vinda do `LoadResObject` já está pronta quando o jogo
                    // registra o callback: a notificação é imediata, não fica esperando
                    // stream nenhum.
                    if self.images.contains_key(&this) {
                        self.notify_image(this)?;
                    }
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Lê o stream inteiro e decodifica o PNG.
    pub(super) fn decode_image(&mut self, image: u32, stream: u32) -> Result<(), CpuError> {
        let Some(source) = self.streams.get(&stream).copied() else {
            return Ok(());
        };
        let bytes = self.read_bytes(source.buffer, source.size)?;
        match decode_png(&bytes) {
            Some(decoded) => {
                self.images.insert(image, std::rc::Rc::new(decoded));
            }
            None => {
                self.assumptions
                    .insert("um PNG do jogo foi recusado pelo decodificador");
            }
        }
        self.notify_image(image)
    }

    /// Enfileira o `PFNIMAGEINFO(pUser, IImage *, AEEImageInfo *, int nErr)` da imagem.
    ///
    /// A imagem já está pronta quando o stream chega — não há decodificação em segundo plano
    /// aqui —, mas o jogo espera a notificação para seguir carregando.
    pub(super) fn notify_image(&mut self, image: u32) -> Result<(), CpuError> {
        let Some(&callback) = self.image_notify.get(&image) else {
            return Ok(());
        };
        let decoded = self.images.get(&image).cloned();
        // `AEEImageInfo` tem 10 bytes; alocamos 12 para manter o alinhamento.
        let info = self.heap.alloc(12).unwrap_or(0);
        if info != 0 {
            self.cpu.write_mem(info, &[0u8; 12])?;
            if let Some(image) = &decoded {
                self.cpu
                    .write_mem(info, &(image.width as u16).to_le_bytes())?;
                self.cpu
                    .write_mem(info + 2, &(image.height as u16).to_le_bytes())?;
                self.cpu
                    .write_mem(info + 8, &image.frame_width.to_le_bytes())?;
            }
        }
        let error = if decoded.is_some() { SUCCESS } else { EFAILED };
        self.pending_calls.push(GuestCall {
            function: callback.function,
            args: [callback.context, image, info, error],
        });
        Ok(())
    }

    /// `IPARM_*` de `inc/AEEIImage.h`. Só respondemos aos que mudam o desenho.
    pub(super) fn image_set_parm(
        &mut self,
        image: u32,
        parm: u32,
        p1: u32,
        p2: u32,
    ) -> Result<(), CpuError> {
        match parm {
            IPARM_CXFRAME => {
                if let Some(info) = self.images.get_mut(&image) {
                    std::rc::Rc::make_mut(info).frame_width = p1 as u16;
                }
            }
            IPARM_NFRAMES => {
                if let Some(info) = self.images.get_mut(&image) {
                    let frames = (p1 as u16).max(1);
                    let width = info.width as u16;
                    std::rc::Rc::make_mut(info).frame_width = width / frames;
                }
            }
            // p1 = ponteiro para receber o `IBitmap *`, p2 = ponteiro para o código de retorno.
            IPARM_GETBITMAP => {
                let bitmap = self.bitmap_from_image(image)?;
                if p1 != 0 {
                    self.cpu.write_u32(p1, bitmap)?;
                }
                if p2 != 0 {
                    self.cpu
                        .write_u32(p2, if bitmap == 0 { EFAILED } else { SUCCESS })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Materializa a imagem decodificada como uma superfície `IBitmap`.
    /// Diferente do [`Machine::bitmap_from_decoded`], este caminho **não** publica o `IDIB`.
    ///
    /// Publicar custa caro onde não é preciso: toda superfície publicada entra no laço que
    /// sincroniza os pixels a cada chamada que os toca, e o Pac-Mania — que desenha pixel a
    /// pixel pela API — saiu de "roda" para "lento demais" quando os dois caminhos foram
    /// unificados. Quem pede o DIB pede pelo `QueryInterface`, e aí ele é publicado.
    pub(super) fn bitmap_from_image(&mut self, image: u32) -> Result<u32, CpuError> {
        // A mesma imagem é pedida de novo a cada quadro — o Pac-Mania faz 34 mil
        // `IPARM_GETBITMAP` em quatro segundos virtuais. Materializar uma superfície nova a
        // cada pedido gastava sete segundos e deixava trinta e quatro mil objetos vivos.
        if let Some(&bitmap) = self.image_bitmaps.get(&image) {
            return Ok(bitmap);
        }
        let Some(info) = self.images.get(&image).cloned() else {
            return Ok(0);
        };
        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let mut surface = Framebuffer::new(info.width, info.height);
        for (index, pixel) in info.pixels.iter().enumerate() {
            let (x, y) = (index as u32 % info.width, index as u32 / info.width);
            surface.set_pixel_native(x as i32, y as i32, *pixel);
        }
        self.bitmaps.insert(addr, surface);
        self.image_bitmaps.insert(image, addr);
        Ok(addr)
    }

    /// Desenha a imagem na superfície corrente.
    ///
    /// Quando o destino é uma superfície do próprio jogo, o desenho não pode ser feito aqui:
    /// ele vira uma chamada ao `BltIn` dela, e entrar no guest no meio do despacho não é
    /// seguro. Vai para a fila da fronteira, como os callbacks.
    pub(super) fn draw_image(
        &mut self,
        image: u32,
        x: i32,
        y: i32,
        frame: Option<u32>,
    ) -> Result<(), CpuError> {
        let Some(info) = self.images.get(&image).cloned() else {
            return Ok(());
        };
        let target = self.target()?;
        if !self.bitmaps.contains_key(&target) {
            self.pending_blits.push(PendingBlit {
                image,
                target,
                x,
                y,
                frame,
            });
            return Ok(());
        }
        let clip = self.clip;
        let Some(surface) = self.bitmaps.get_mut(&target) else {
            return Ok(());
        };
        let (frame_width, offset) = match (frame, info.frame_width) {
            (Some(n), width) if width > 0 => (width as u32, n * width as u32),
            _ => (info.width, 0),
        };
        // O recorte não é acabamento aqui: é o que decide o tamanho do trabalho. O Pac-Mania
        // desenha a **folha de fontes inteira** e conta com o recorte para que só a letra
        // apareça. Percorrer a imagem toda e conferir pixel a pixel eram 3,9 bilhões de pixels
        // lidos em quatro segundos virtuais para pôr na tela algumas centenas de milhares — e
        // ainda punha na tela o que o jogo mandou esconder.
        let (mut first_column, mut last_column) = (0, frame_width as i32);
        let (mut first_row, mut last_row) = (0, info.height as i32);
        if let Some(clip) = clip {
            first_column = first_column.max(clip.x as i32 - x);
            last_column = last_column.min(clip.x as i32 + clip.width as i32 - x);
            first_row = first_row.max(clip.y as i32 - y);
            last_row = last_row.min(clip.y as i32 + clip.height as i32 - y);
        }
        // O mesmo vale para as bordas da superfície: o que cai fora nunca precisou ser lido.
        first_column = first_column.max(-x).max(0);
        last_column = last_column.min(surface.width() as i32 - x);
        first_row = first_row.max(-y).max(0);
        last_row = last_row.min(surface.height() as i32 - y);

        for row in first_row..last_row {
            for column in first_column..last_column {
                let source = (row as u32 * info.width + column as u32 + offset) as usize;
                if let Some(&pixel) = info.pixels.get(source) {
                    if info.opaque.get(source).copied().unwrap_or(true) {
                        surface.set_pixel_native(x + column, y + row, pixel);
                    }
                }
            }
        }
        Ok(())
    }
}
