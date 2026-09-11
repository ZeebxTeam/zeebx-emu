//! IDisplay e IGraphics: o desenho 2D e o texto na tela.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Métodos de `IDisplay`. Despachamos pelo **nome** do slot, não pelo número: a tabela de
    /// nomes vem dos headers do SDK, então um método fora de ordem viraria erro de compilação
    /// aqui em vez de desenho errado lá.
    pub(super) fn display_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Display.method(slot) else {
            return Ok(None);
        };
        let result = match name {
            // O `IDisplay` não tem contagem própria: o objeto é a tela, e ela vive enquanto o
            // emulador viver. Contamos as referências só para o jogo ver o número que espera.
            "AddRef" => self.objects.add_ref(self.cpu.read_reg(Reg::R0)),
            "Release" => self.objects.release(self.cpu.read_reg(Reg::R0)),
            // RGBVAL IDISPLAY_SetColor(IDisplay *p, AEEClrItem clr, RGBVAL rgb)
            "SetColor" => {
                let item = self.cpu.read_reg(Reg::R1) as usize;
                let previous = self.colors.get(item).copied().unwrap_or(Rgb::BLACK);
                if let Some(slot) = self.colors.get_mut(item) {
                    *slot = Rgb::from_rgbval(self.cpu.read_reg(Reg::R2));
                }
                to_rgbval(previous)
            }
            // void IDISPLAY_DrawRect(IDisplay *p, const AEERect *pr, RGBVAL cf, RGBVAL cfill,
            //                        uint32 flags)
            "DrawRect" => {
                let target = self.target()?;
                // **Ponteiro nulo é a superfície inteira.** É como o `ClearScreen` do SDK é
                // escrito, e é a única forma de limpar a tela que o BREW oferece.
                let pedido = match self.read_rect(self.cpu.read_reg(Reg::R1))? {
                    Some(rect) => Some(rect),
                    None => self.bitmaps.get(&target).map(|fb| Rect {
                        x: 0,
                        y: 0,
                        width: fb.width().min(i16::MAX as u32) as i16,
                        height: fb.height().min(i16::MAX as u32) as i16,
                    }),
                };
                let rect = pedido.and_then(|rect| self.clip_rect(rect));
                // `RGB_NONE` quer dizer "a cor corrente" — a de fundo para o preenchimento e a
                // de linha para a moldura, ambas do `IDISPLAY_SetColor`.
                let cor = |valor: u32, item: usize, cores: &[Rgb]| match valor {
                    RGB_NONE => cores[item],
                    valor => Rgb::from_rgbval(valor),
                };
                let border = cor(self.cpu.read_reg(Reg::R2), CLR_USER_LINE, &self.colors);
                let fill = cor(
                    self.cpu.read_reg(Reg::R3),
                    CLR_USER_BACKGROUND,
                    &self.colors,
                );
                // **Quem manda no que é desenhado é o `flags`, não a cor.** O Quake pede moldura
                // sozinha (`IDF_RECT_FRAME`) passando preto no preenchimento: honrar a cor e
                // ignorar o sinalizador pintava um retângulo preto que ele não pediu.
                //
                // Sinalizador nenhum mantém o comportamento antigo — desenhar os dois. Nenhum
                // jogo do acervo chama assim, e na dúvida é melhor continuar desenhando do que
                // apagar uma tela por causa de uma leitura que não deu para conferir.
                let flags = match self.stack_arg(0)? {
                    0 => IDF_RECT_FRAME | IDF_RECT_FILL,
                    flags => flags,
                };
                if let (Some(rect), Some(fb)) = (rect, self.bitmaps.get_mut(&target)) {
                    if flags & IDF_RECT_FILL != 0 {
                        fb.fill_rect(rect, fill);
                    }
                    if flags & IDF_RECT_FRAME != 0 {
                        fb.draw_frame(rect, border);
                    }
                }
                SUCCESS
            }
            // int IDISPLAY_GetDeviceBitmap(IDisplay *p, IBitmap **ppIBitmap).
            //
            // Devolve **código de erro** e entrega a superfície pelo ponteiro de saída, ao
            // contrário do `GetDestination` logo abaixo, que devolve o `IBitmap *` direto. Eu
            // tinha implementado os dois iguais, e o Bejeweled Twist — que testa
            // `if (retorno != 0) falhou` — desistia da inicialização por causa disso.
            "GetDeviceBitmap" => {
                let out = self.cpu.read_reg(Reg::R1);
                let bitmap = self.device_bitmap()?;
                if out != 0 {
                    self.cpu.write_u32(out, bitmap)?;
                }
                if bitmap == 0 {
                    ENOMEMORY
                } else {
                    // Devolve uma referência: o jogo dá `Release` quando termina, e faz isso
                    // uma vez por quadro. Sem o `AddRef` a contagem zerava e a superfície da
                    // tela era destruída no meio da execução.
                    self.objects.add_ref(bitmap);
                    SUCCESS
                }
            }
            // int SetDestination(IDisplay *, IBitmap *pbmDest)
            //
            // O destino nem sempre é uma superfície nossa: o Bejeweled Twist implementa o
            // próprio `IBitmap`, com a vtable embutida no objeto (o `DECLARE_VTBL` do BREW).
            // Para desenhar nela é preciso perguntar a ela onde ficam os pixels — o que exige
            // entrar no guest, e por isso fica para a fronteira da chamada.
            "SetDestination" => {
                let target = self.cpu.read_reg(Reg::R1);

                if target != 0 && !self.bitmaps.contains_key(&target) && self.probed.insert(target)
                {
                    self.pending_probes.push(target);
                }
                self.display_target = target;
                SUCCESS
            }
            "GetDestination" => self.target()?,
            // void IDISPLAY_BitBlt(IDisplay *p, int xd, int yd, int w, int h,
            //                      const void *pbmSource, int xs, int ys, AEERasterOp rop)
            "BitBlt" => {
                let dst_pos = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let src = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                let target = self.target()?;
                if let Some((dst_pos, size, origin)) = self.clip_blit(dst_pos, size, origin) {
                    self.blit(target, dst_pos, size, src, origin, rop);
                }
                SUCCESS
            }
            // void IDISPLAY_DrawText(IDisplay *, AEEFont, const AECHAR *pcText, int nChars,
            //                        int x, int y, const AEERect *prcBackground, uint32 dwFlags)
            "DrawText" => {
                let text = self.read_aechar(self.cpu.read_reg(Reg::R2))?;
                let (x, y) = (self.stack_arg(0)? as i32, self.stack_arg(1)? as i32);
                match self.draw_text(&text, x, y)? {
                    true => {}
                    // Sem fonte, o texto continua indo só para o relatório: é o que permite
                    // saber que o jogo *quer* escrever mesmo quando não há com o quê.
                    false => {
                        // A comparação só roda enquanto há espaço; cheia, a lista custa um
                        // teste de tamanho por chamada.
                        if self.pending_text.len() < MAX_TEXT && !self.pending_text.contains(&text)
                        {
                            self.pending_text.push(text);
                        }
                    }
                }
                SUCCESS
            }
            // Sem efeito visível para nós: o framebuffer já está sempre atualizado.
            // void IDISPLAY_GetClipRect(IDisplay *p, AEERect *prc)
            //
            // O recorte é ignorado no desenho, então o retângulo corrente é a tela inteira —
            // que é o que o BREW devolve quando ninguém apertou o recorte. O Pac-Mania lê isto
            // para guardar e restaurar depois.
            "GetClipRect" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    let rect = self.clip.unwrap_or_else(|| {
                        let screen = self.screen();
                        Rect {
                            x: 0,
                            y: 0,
                            width: screen.width() as i16,
                            height: screen.height() as i16,
                        }
                    });
                    let mut bytes = [0u8; 8];
                    bytes[0..2].copy_from_slice(&rect.x.to_le_bytes());
                    bytes[2..4].copy_from_slice(&rect.y.to_le_bytes());
                    bytes[4..6].copy_from_slice(&rect.width.to_le_bytes());
                    bytes[6..8].copy_from_slice(&rect.height.to_le_bytes());
                    self.cpu.write_mem(out, &bytes)?;
                }
                SUCCESS
            }
            // int IDISPLAY_GetFontMetrics(IDisplay *p, AEEFont font, int *pnAscent,
            //                              int *pnDescent)
            //
            // Devolve a altura da fonte. Ainda não desenhamos texto, mas os jogos medem antes
            // de posicionar: sem números aqui, o Pac-Mania nem chega a montar a tela. Os
            // valores são os de uma fonte de tela pequena, coerentes entre si.
            // int IDISPLAY_MeasureTextEx(IDisplay *, AEEFont, const AECHAR *pcText,
            //                             int nChars, int nMaxWidth, int *pnFits)
            //
            // Devolve a largura em pixels e, em `pnFits`, quantos caracteres cabem em
            // `nMaxWidth`. Como ainda não desenhamos texto, a largura sai de um avanço fixo por
            // caractere, coerente com a altura que o `GetFontMetrics` informa. É medida
            // aproximada de propósito: serve para o jogo centralizar e quebrar linha, e um
            // número plausível o deixa seguir — sem nenhum, o Pac-Mania para na escolha do
            // idioma.
            "MeasureTextEx" => {
                let text = self.read_aechar_units(self.cpu.read_reg(Reg::R2))?;
                let chars = self.cpu.read_reg(Reg::R3) as i32;
                let count = match chars < 0 {
                    true => text.len(),
                    false => (chars as usize).min(text.len()),
                };
                let max_width = self.stack_arg(0)? as i32;
                // Com fonte, a largura é medida caractere a caractere até estourar o limite;
                // sem ela sobra a largura fixa, que é chute honesto mas chute.
                let (fits, largura) = match &self.font {
                    Some(fonte) => {
                        let mut cabem = 0;
                        let mut largura = 0;
                        for fim in 1..=count {
                            let trecho: String = text[..fim]
                                .iter()
                                .filter_map(|u| char::from_u32(u32::from(*u)))
                                .collect();
                            let candidata = fonte.width(&trecho, FONT_SIZE) as i32;
                            if max_width >= 0 && candidata > max_width {
                                break;
                            }
                            (cabem, largura) = (fim, candidata);
                        }
                        (cabem, largura as u32)
                    }
                    None => {
                        let cabem = match max_width < 0 {
                            true => count,
                            false => count.min((max_width / FONT_ADVANCE).max(0) as usize),
                        };
                        (cabem, (cabem as i32 * FONT_ADVANCE) as u32)
                    }
                };
                let out = self.stack_arg(1)?;
                if out != 0 {
                    self.cpu.write_u32(out, fits as u32)?;
                }
                largura
            }
            "GetFontMetrics" => {
                // Com fonte de verdade, a medida é dela: um menu que centraliza pela altura da
                // linha fica torto se a altura for chute.
                let (ascent, descent) = match &self.font {
                    Some(fonte) => (fonte.ascent(FONT_SIZE), fonte.descent(FONT_SIZE)),
                    None => (FONT_ASCENT, FONT_DESCENT),
                };
                self.write_at(self.cpu.read_reg(Reg::R2), ascent)?;
                self.write_at(self.cpu.read_reg(Reg::R3), descent)?;
                ascent + descent
            }
            // int IDISPLAY_SetClipRect(IDisplay *p, const AEERect *prc)
            //
            // Ponteiro nulo volta ao recorte cheio, que é o que o BREW define. Ignorar esta
            // chamada custava caro: o Pac-Mania desenha a folha de fontes inteira e conta com
            // o recorte para que só a letra apareça — sem ele, a folha toda ia para a tela.
            "SetClipRect" => {
                self.clip = self.read_rect(self.cpu.read_reg(Reg::R1))?;
                SUCCESS
            }
            // int IDISPLAY_Clone(IDisplay *po, IDisplay **ppNew)
            //
            // Uma cópia do objeto de tela, para o app desenhar noutro estado sem mexer no
            // corrente. A tela é uma só, então o que sai daqui é o mesmo objeto com uma
            // referência a mais — é o que o BREW faz quando o dispositivo tem uma tela.
            "Clone" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out == 0 {
                    return Ok(Some(EBADPARM));
                }
                // Um objeto novo, não o mesmo com uma referência a mais: o app solta a cópia
                // quando termina com ela, e devolver o original faria essa soltura derrubar a
                // tela que ele ainda usa. O estado de desenho é da `Machine`, não do objeto,
                // então dois objetos apontam para a mesma tela sem se atrapalharem.
                let clone = self.new_object(Interface::Display)?;
                if clone == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.cpu.write_u32(out, clone)?;
                SUCCESS
            }
            "Update" | "SetFont" | "SetAnnunciators" | "Backlight" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Superfície onde o `IDisplay` desenha.
    pub(super) fn target(&mut self) -> Result<u32, CpuError> {
        if self.display_target == 0 {
            return self.device_bitmap();
        }
        Ok(self.display_target)
    }

    /// `IGraphics` — a API 2D do BREW, desenhada sobre a mesma superfície do `IDisplay`.
    ///
    /// O estado (cor de traço, cor de preenchimento, se preenche ou não, translação) fica no
    /// host; as primitivas viram operações no framebuffer.
    pub(super) fn graphics_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Graphics.method(slot) else {
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
            "Release" => self.objects.release(this),
            // As cores chegam como três ou quatro `uint8` soltos, não como RGBVAL.
            "SetBackground" => {
                let previous = self.graphics.background;
                self.graphics.background = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetColor" => {
                let previous = self.graphics.stroke;
                self.graphics.stroke = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetFillColor" => {
                let previous = self.graphics.fill;
                self.graphics.fill = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetFillMode" => {
                let previous = self.graphics.fill_mode;
                self.graphics.fill_mode = a1 != 0;
                u32::from(previous)
            }
            "GetFillMode" => u32::from(self.graphics.fill_mode),
            "SetPointSize" => {
                let previous = self.graphics.point_size;
                self.graphics.point_size = a1 as u8;
                previous as u32
            }
            "GetPointSize" => self.graphics.point_size as u32,
            "GetColorDepth" => COLOR_DEPTH as u32,
            "Translate" => {
                self.graphics.origin = (a1 as i16 as i32, a2 as i16 as i32);
                SUCCESS
            }
            // int DrawPoint(IGraphics *, AEEPoint *) — AEEPoint é { int16 x, y }.
            "DrawPoint" => {
                let (x, y) = self.read_point(a1)?;
                let (x, y) = self.translated(x, y);
                let color = self.graphics.stroke;
                self.with_target(|fb| fb.set_pixel(x, y, color))?;
                SUCCESS
            }
            // int DrawLine(IGraphics *, AEELine *) — AEELine é { int16 sx, sy, ex, ey }.
            "DrawLine" => {
                let mut bytes = [0u8; 8];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let (sx, sy) = self.translated(read(0), read(2));
                let (ex, ey) = self.translated(read(4), read(6));
                let color = self.graphics.stroke;
                self.with_target(|fb| fb.draw_line(sx, sy, ex, ey, color))?;
                SUCCESS
            }
            "DrawRect" | "ClearRect" => {
                let Some(rect) = self.read_rect(a1)? else {
                    return Ok(Some(EBADPARM));
                };
                let rect = self.translated_rect(rect);
                let (stroke, fill, filled) = (
                    self.graphics.stroke,
                    self.graphics.fill,
                    self.graphics.fill_mode,
                );
                // `ClearRect` pinta com a cor de fundo; `DrawRect` respeita o modo de
                // preenchimento e sempre desenha a borda.
                if name == "ClearRect" {
                    let background = self.graphics.background;
                    self.with_target(|fb| fb.fill_rect(rect, background))?;
                } else {
                    self.with_target(|fb| {
                        if filled {
                            fb.fill_rect(rect, fill);
                        }
                        fb.draw_frame(rect, stroke);
                    })?;
                }
                SUCCESS
            }
            // AEECircle é { int16 cx, cy, r }.
            "DrawCircle" => {
                let mut bytes = [0u8; 6];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let (cx, cy) = self.translated(read(0), read(2));
                let radius = read(4);
                let (stroke, fill, filled) = (
                    self.graphics.stroke,
                    self.graphics.fill,
                    self.graphics.fill_mode,
                );
                self.with_target(|fb| {
                    if filled {
                        fb.fill_circle(cx, cy, radius, fill);
                    }
                    fb.draw_circle(cx, cy, radius, stroke);
                })?;
                SUCCESS
            }
            // AEETriangle é { int16 x0, y0, x1, y1, x2, y2 }.
            "DrawTriangle" => {
                let mut bytes = [0u8; 12];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let points: Vec<(i32, i32)> = (0..3)
                    .map(|i| self.translated(read(i * 4), read(i * 4 + 2)))
                    .collect();
                self.draw_shape(&points, true)?;
                SUCCESS
            }
            // AEEPolygon e AEEPolyline são { int16 len; AEEPoint *points }.
            "DrawPolygon" | "DrawPolyline" => {
                let count = self.cpu.read_u32(a1)? as u16 as usize;
                let array = self.cpu.read_u32(a1 + 4)?;
                let mut points = Vec::with_capacity(count);
                for i in 0..count.min(MAX_POLYGON_POINTS) {
                    let (x, y) = self.read_point(array + i as u32 * 4)?;
                    points.push(self.translated(x, y));
                }
                self.draw_shape(&points, name == "DrawPolygon")?;
                SUCCESS
            }
            "ClearViewport" => {
                let background = self.graphics.background;
                let rect = Rect {
                    x: 0,
                    y: 0,
                    width: SCREEN_WIDTH as i16,
                    height: SCREEN_HEIGHT as i16,
                };
                self.with_target(|fb| fb.fill_rect(rect, background))?;
                SUCCESS
            }
            "SetDestination" => {
                self.display_target = a1;
                SUCCESS
            }
            "GetDestination" => self.target()?,
            // Sem efeito para nós: já desenhamos direto na superfície.
            "Update" | "EnableDoubleBuffer" | "SetPaintMode" | "SetClip" | "SetViewport"
            | "SetAlgorithmHint" | "SetStrokeStyle" | "Pan" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Desenha um polígono fechado (ou uma polilinha aberta) com o estado atual.
    pub(super) fn draw_shape(&mut self, points: &[(i32, i32)], closed: bool) -> Result<(), CpuError> {
        if points.is_empty() {
            return Ok(());
        }
        let (stroke, fill, filled) = (
            self.graphics.stroke,
            self.graphics.fill,
            self.graphics.fill_mode && closed,
        );
        let points = points.to_vec();
        self.with_target(move |fb| {
            if filled {
                fb.fill_polygon(&points, fill);
            }
            let last = if closed {
                points.len()
            } else {
                points.len() - 1
            };
            for i in 0..last {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % points.len()];
                fb.draw_line(x0, y0, x1, y1, stroke);
            }
        })
    }

    /// Executa uma operação de desenho na superfície corrente.
    pub(super) fn with_target(&mut self, draw: impl FnOnce(&mut Framebuffer)) -> Result<(), CpuError> {
        let target = self.target()?;
        if let Some(fb) = self.bitmaps.get_mut(&target) {
            draw(fb);
        }
        Ok(())
    }

    /// Lê um `AEEPoint` — dois `int16`.
    pub(super) fn read_point(&self, addr: u32) -> Result<(i32, i32), CpuError> {
        let mut bytes = [0u8; 4];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok((
            i16::from_le_bytes([bytes[0], bytes[1]]) as i32,
            i16::from_le_bytes([bytes[2], bytes[3]]) as i32,
        ))
    }

    /// Aplica a translação corrente do `IGraphics`.
    pub(super) fn translated(&self, x: i32, y: i32) -> (i32, i32) {
        (x + self.graphics.origin.0, y + self.graphics.origin.1)
    }

    pub(super) fn translated_rect(&self, rect: Rect) -> Rect {
        Rect {
            x: rect.x + self.graphics.origin.0 as i16,
            y: rect.y + self.graphics.origin.1 as i16,
            ..rect
        }
    }

    /// Desenha `text` na superfície corrente. `false` quando não há fonte para desenhá-lo.
    ///
    /// A cor é a do `CLR_USER_TEXT`, que é o que o `IDISPLAY_SetColor` ajusta, e o recorte vale
    /// aqui como em qualquer outro desenho.
    pub(super) fn draw_text(&mut self, text: &str, x: i32, y: i32) -> Result<bool, CpuError> {
        let Some(fonte) = self.font.as_ref() else {
            return Ok(false);
        };
        let glifos = fonte.layout(text, FONT_SIZE);
        if glifos.is_empty() {
            return Ok(true);
        }
        let cor = self
            .colors
            .get(CLR_USER_TEXT)
            .copied()
            .unwrap_or(Rgb::BLACK);
        self.escreve(text, x, y, cor)
    }

    /// Escreve com a fonte carregada, numa cor dada. É o miolo do [`Machine::draw_text`],
    /// separado porque o widget traz a cor dele na propriedade e não usa a da paleta.
    pub(super) fn escreve(&mut self, text: &str, x: i32, y: i32, cor: Rgb) -> Result<bool, CpuError> {
        let Some(fonte) = self.font.as_ref() else {
            return Ok(false);
        };
        let glifos = fonte.layout(text, FONT_SIZE);
        if glifos.is_empty() {
            return Ok(true);
        }
        let nativo = cor.to_rgb565();
        let recorte = self.clip;
        let target = self.target()?;
        let Some(surface) = self.bitmaps.get_mut(&target) else {
            return Ok(true);
        };
        for glifo in glifos {
            for linha in 0..glifo.height {
                for coluna in 0..glifo.width {
                    // Meio-tom não existe numa superfície sem canal alfa: ou a letra cobre o
                    // pixel, ou não cobre. Metade é o corte que deixa a borda parecida com a
                    // do console, que também não mistura.
                    if glifo.coverage[(linha * glifo.width + coluna) as usize] < 128 {
                        continue;
                    }
                    let (px, py) = (x + glifo.x + coluna as i32, y + glifo.y + linha as i32);
                    if let Some(clip) = recorte {
                        let dentro = px >= clip.x as i32
                            && py >= clip.y as i32
                            && px < clip.x as i32 + clip.width as i32
                            && py < clip.y as i32 + clip.height as i32;
                        if !dentro {
                            continue;
                        }
                    }
                    surface.set_pixel_native(px, py, nativo);
                }
            }
        }
        Ok(true)
    }

    /// A tela, como está agora.
    pub fn screen(&self) -> &Framebuffer {
        self.bitmaps
            .get(&self.device_bitmap)
            .unwrap_or(&self.screen)
    }

    /// Textos que o jogo pediu para desenhar mas que ainda não sabemos rasterizar.
    pub fn pending_text(&self) -> &[String] {
        &self.pending_text
    }
}
