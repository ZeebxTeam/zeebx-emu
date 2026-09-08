//! Framebuffer do console e as operações de desenho do `IDisplay`.
//!
//! A tela do Zeebo é VGA 640×480 em RGB565 — o mesmo formato do framebuffer que a engenharia
//! reversa capturou do encoder de TV do console (`docs/vendor/tripleoxygen/research/tvenc/`).
//! Guardamos os pixels nesse formato para que um `BitBlt` de bitmap nativo seja cópia direta.

/// Cor no formato que o BREW usa em `RGBVAL`: `MAKE_RGB(r,g,b) = (r<<8) | (g<<16) | (b<<24)`,
/// conforme a documentação do SDK — ou seja, o byte menos significativo não é cor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };

    /// Converte um `RGBVAL` do guest.
    pub fn from_rgbval(value: u32) -> Self {
        Self {
            r: (value >> 8) as u8,
            g: (value >> 16) as u8,
            b: (value >> 24) as u8,
        }
    }

    /// Empacota em RGB565, o formato do framebuffer.
    pub fn to_rgb565(self) -> u16 {
        ((self.r as u16 & 0xf8) << 8) | ((self.g as u16 & 0xfc) << 3) | (self.b as u16 >> 3)
    }

    pub fn from_rgb565(value: u16) -> Self {
        // Replica os bits altos nos baixos para que branco puro continue 255, e não 248.
        let r = ((value >> 11) & 0x1f) as u8;
        let g = ((value >> 5) & 0x3f) as u8;
        let b = (value & 0x1f) as u8;
        Self {
            r: (r << 3) | (r >> 2),
            g: (g << 2) | (g >> 4),
            b: (b << 3) | (b >> 2),
        }
    }
}

/// Retângulo do BREW (`AEERect`): quatro `int16` — x, y, largura, altura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub width: i16,
    pub height: i16,
}

impl Rect {
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]);
        Self {
            x: read(0),
            y: read(2),
            width: read(4),
            height: read(6),
        }
    }
}

#[derive(Debug)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    pixels: Vec<u16>,
    /// Quantos pixels já foram efetivamente escritos — serve para saber se há algo a mostrar.
    touched: u64,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height) as usize],
            touched: 0,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Se alguma operação de desenho chegou a tocar a tela.
    pub fn is_dirty(&self) -> bool {
        self.touched > 0
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, color: Rgb) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.pixels[(y as u32 * self.width + x as u32) as usize] = color.to_rgb565();
        self.touched += 1;
    }

    /// Preenche um retângulo, recortando o que sair da tela.
    pub fn fill_rect(&mut self, rect: Rect, color: Rgb) {
        let value = color.to_rgb565();
        let x0 = rect.x.max(0) as u32;
        let y0 = rect.y.max(0) as u32;
        let x1 = ((rect.x as i32 + rect.width as i32).max(0) as u32).min(self.width);
        let y1 = ((rect.y as i32 + rect.height as i32).max(0) as u32).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                self.pixels[(y * self.width + x) as usize] = value;
                self.touched += 1;
            }
        }
    }

    /// Desenha a moldura de um retângulo, um pixel de espessura.
    pub fn draw_frame(&mut self, rect: Rect, color: Rgb) {
        let (x, y) = (rect.x as i32, rect.y as i32);
        let (w, h) = (rect.width as i32, rect.height as i32);
        if w <= 0 || h <= 0 {
            return;
        }
        for i in 0..w {
            self.set_pixel(x + i, y, color);
            self.set_pixel(x + i, y + h - 1, color);
        }
        for i in 0..h {
            self.set_pixel(x, y + i, color);
            self.set_pixel(x + w - 1, y + i, color);
        }
    }

    /// Lê um pixel no formato nativo (RGB565). Fora da tela devolve zero.
    pub fn get_pixel(&self, x: i32, y: i32) -> u16 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.pixels[(y as u32 * self.width + x as u32) as usize]
    }

    /// Escreve um pixel já em formato nativo.
    pub fn set_pixel_native(&mut self, x: i32, y: i32, value: u16) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.pixels[(y as u32 * self.width + x as u32) as usize] = value;
        self.touched += 1;
    }

    /// Preenche um retângulo com cor já em formato nativo.
    pub fn fill_rect_native(&mut self, rect: Rect, value: u16) {
        let x1 = (rect.x as i32 + rect.width as i32)
            .max(0)
            .min(self.width as i32);
        let y1 = (rect.y as i32 + rect.height as i32)
            .max(0)
            .min(self.height as i32);
        for y in rect.y.max(0) as i32..y1 {
            for x in rect.x.max(0) as i32..x1 {
                self.pixels[(y as u32 * self.width + x as u32) as usize] = value;
                self.touched += 1;
            }
        }
    }

    /// Copia uma região de `src` para dentro desta superfície.
    ///
    /// `transparent` marca a cor que deve ser pulada — é assim que o BREW faz sprite com
    /// `AEE_RO_TRANSPARENT`. Passar `None` copia tudo.
    ///
    /// A assinatura é larga de propósito: ela espelha o `BltIn` do BREW, e reagrupar os
    /// parâmetros em structs só afastaria o código da API que ele implementa.
    #[allow(clippy::too_many_arguments)]
    pub fn blit(
        &mut self,
        dst_x: i32,
        dst_y: i32,
        width: i32,
        height: i32,
        src: &Framebuffer,
        src_x: i32,
        src_y: i32,
        transparent: Option<u16>,
    ) {
        for row in 0..height {
            for col in 0..width {
                let (sx, sy) = (src_x + col, src_y + row);
                if sx < 0 || sy < 0 || sx >= src.width as i32 || sy >= src.height as i32 {
                    continue;
                }
                let value = src.get_pixel(sx, sy);
                if Some(value) == transparent {
                    continue;
                }
                self.set_pixel_native(dst_x + col, dst_y + row, value);
            }
        }
    }

    /// Traça uma linha pelo algoritmo de Bresenham — só inteiros, como o hardware da época.
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb) {
        let value = color.to_rgb565();
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut error) = (x0, y0, dx + dy);
        loop {
            self.set_pixel_native(x, y, value);
            if x == x1 && y == y1 {
                break;
            }
            let doubled = error * 2;
            if doubled >= dy {
                error += dy;
                x += sx;
            }
            if doubled <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    /// Circunferência pelo algoritmo do ponto médio, espelhando os oito octantes.
    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Rgb) {
        if radius < 0 {
            return;
        }
        let value = color.to_rgb565();
        let (mut x, mut y) = (radius, 0);
        let mut error = 1 - radius;
        while x >= y {
            for (px, py) in [
                (cx + x, cy + y),
                (cx + y, cy + x),
                (cx - y, cy + x),
                (cx - x, cy + y),
                (cx - x, cy - y),
                (cx - y, cy - x),
                (cx + y, cy - x),
                (cx + x, cy - y),
            ] {
                self.set_pixel_native(px, py, value);
            }
            y += 1;
            if error < 0 {
                error += 2 * y + 1;
            } else {
                x -= 1;
                error += 2 * (y - x) + 1;
            }
        }
    }

    /// Preenche um círculo varrendo linha a linha.
    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Rgb) {
        let value = color.to_rgb565();
        for dy in -radius..=radius {
            let half = ((radius * radius - dy * dy) as f64).sqrt() as i32;
            for dx in -half..=half {
                self.set_pixel_native(cx + dx, cy + dy, value);
            }
        }
    }

    /// Preenche um polígono pela regra de paridade das interseções com cada linha.
    pub fn fill_polygon(&mut self, points: &[(i32, i32)], color: Rgb) {
        if points.len() < 3 {
            return;
        }
        let value = color.to_rgb565();
        let top = points.iter().map(|p| p.1).min().unwrap_or(0).max(0);
        let bottom = points
            .iter()
            .map(|p| p.1)
            .max()
            .unwrap_or(0)
            .min(self.height as i32 - 1);
        for y in top..=bottom {
            let mut crossings: Vec<i32> = Vec::new();
            for i in 0..points.len() {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % points.len()];
                if (y0 <= y && y1 > y) || (y1 <= y && y0 > y) {
                    let t = (y - y0) as f64 / (y1 - y0) as f64;
                    crossings.push(x0 + (t * (x1 - x0) as f64) as i32);
                }
            }
            crossings.sort_unstable();
            for pair in crossings.chunks(2) {
                if let [start, end] = pair {
                    for x in *start..=*end {
                        self.set_pixel_native(x, y, value);
                    }
                }
            }
        }
    }

    /// Os pixels em bytes RGB565, na ordem em que o `IDIB` os expõe ao jogo.
    pub fn to_rgb565_bytes(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    /// Recarrega os pixels a partir do buffer que o jogo escreveu.
    pub fn load_rgb565_bytes(&mut self, bytes: &[u8]) {
        for (pixel, chunk) in self.pixels.iter_mut().zip(bytes.chunks_exact(2)) {
            let value = u16::from_le_bytes([chunk[0], chunk[1]]);
            if *pixel != value {
                *pixel = value;
                self.touched += 1;
            }
        }
    }

    /// Os pixels em `0x00RRGGBB`, que é o formato que as janelas do host esperam.
    pub fn to_argb(&self) -> Vec<u32> {
        self.pixels
            .iter()
            .map(|&p| {
                // Repetir os bits mais altos nos que faltam espalha o valor por toda a faixa:
                // é o que faz 0b11111 virar 255 e não 248.
                let r = ((p >> 11) & 0x1f) as u32;
                let g = ((p >> 5) & 0x3f) as u32;
                let b = (p & 0x1f) as u32;
                (((r << 3) | (r >> 2)) << 16) | (((g << 2) | (g >> 4)) << 8) | ((b << 3) | (b >> 2))
            })
            .collect()
    }

    /// Inverte os bits de um retângulo — o `AEE_RO_XOR` do BREW.
    pub fn xor_rect_native(&mut self, rect: Rect, value: u16) {
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let (x, y) = (x as i32, y as i32);
                let current = self.get_pixel(x, y);
                self.set_pixel_native(x, y, current ^ value);
            }
        }
    }

    /// Serializa como BMP de 24 bits — formato que qualquer visualizador abre e que dá para
    /// escrever sem dependência nenhuma.
    pub fn to_bmp(&self) -> Vec<u8> {
        // Cada linha do BMP é alinhada em 4 bytes, e as linhas vão de baixo para cima.
        let row_padding = (4 - (self.width as usize * 3) % 4) % 4;
        let row_size = self.width as usize * 3 + row_padding;
        let pixel_data = row_size * self.height as usize;
        const HEADER: usize = 54;

        let mut out = Vec::with_capacity(HEADER + pixel_data);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((HEADER + pixel_data) as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // reservado
        out.extend_from_slice(&(HEADER as u32).to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes()); // tamanho do BITMAPINFOHEADER
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(self.height as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // planos
        out.extend_from_slice(&24u16.to_le_bytes()); // bits por pixel
        out.extend_from_slice(&0u32.to_le_bytes()); // sem compressão
        out.extend_from_slice(&(pixel_data as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes()); // ~72 dpi
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // cores na paleta
        out.extend_from_slice(&0u32.to_le_bytes()); // cores importantes

        for y in (0..self.height).rev() {
            for x in 0..self.width {
                let color = Rgb::from_rgb565(self.pixels[(y * self.width + x) as usize]);
                out.extend_from_slice(&[color.b, color.g, color.r]);
            }
            out.extend(std::iter::repeat_n(0u8, row_padding));
        }
        out
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_conversao_para_a_janela_espalha_os_bits_ate_o_fim_da_faixa() {
        let mut fb = Framebuffer::new(2, 1);
        fb.load_rgb565_bytes(&[0xff, 0xff, 0x00, 0x00]);
        // Branco em RGB565 tem de virar branco de verdade, não 0xf8fcf8.
        assert_eq!(fb.to_argb(), vec![0x00ff_ffff, 0x0000_0000]);
    }
    use super::*;

    #[test]
    fn converte_rgbval_do_guest() {
        // MAKE_RGB(0x12, 0x34, 0x56) = 0x56341200
        let color = Rgb::from_rgbval(0x5634_1200);
        assert_eq!(
            color,
            Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
    }

    #[test]
    fn rgb565_preserva_branco_e_preto() {
        assert_eq!(Rgb::BLACK.to_rgb565(), 0x0000);
        assert_eq!(Rgb::WHITE.to_rgb565(), 0xffff);
        assert_eq!(Rgb::from_rgb565(0xffff), Rgb::WHITE);
        assert_eq!(Rgb::from_rgb565(0x0000), Rgb::BLACK);
    }

    #[test]
    fn preenche_retangulo_recortando_na_borda() {
        let mut fb = Framebuffer::new(4, 4);
        fb.fill_rect(
            Rect {
                x: 2,
                y: 2,
                width: 10,
                height: 10,
            },
            Rgb::WHITE,
        );
        assert_eq!(
            fb.pixels,
            vec![
                0, 0, 0, 0, //
                0, 0, 0, 0, //
                0, 0, 0xffff, 0xffff, //
                0, 0, 0xffff, 0xffff,
            ]
        );
    }

    #[test]
    fn retangulo_fora_da_tela_nao_estoura() {
        let mut fb = Framebuffer::new(4, 4);
        fb.fill_rect(
            Rect {
                x: -20,
                y: -20,
                width: 5,
                height: 5,
            },
            Rgb::WHITE,
        );
        fb.fill_rect(
            Rect {
                x: 100,
                y: 100,
                width: 5,
                height: 5,
            },
            Rgb::WHITE,
        );
        assert!(!fb.is_dirty());
    }

    #[test]
    fn le_aeerect_do_guest() {
        let rect = Rect::from_bytes([10, 0, 20, 0, 30, 0, 40, 0]);
        assert_eq!(
            rect,
            Rect {
                x: 10,
                y: 20,
                width: 30,
                height: 40
            }
        );
    }

    #[test]
    fn linha_horizontal_pinta_todos_os_pixels() {
        let mut fb = Framebuffer::new(8, 8);
        fb.draw_line(1, 3, 6, 3, Rgb::WHITE);
        for x in 1..=6 {
            assert_eq!(fb.get_pixel(x, 3), 0xffff, "faltou o pixel em x={x}");
        }
        assert_eq!(fb.get_pixel(0, 3), 0);
        assert_eq!(fb.get_pixel(7, 3), 0);
    }

    #[test]
    fn linha_diagonal_liga_as_duas_pontas() {
        let mut fb = Framebuffer::new(8, 8);
        fb.draw_line(0, 0, 7, 7, Rgb::WHITE);
        assert_eq!(fb.get_pixel(0, 0), 0xffff);
        assert_eq!(fb.get_pixel(4, 4), 0xffff);
        assert_eq!(fb.get_pixel(7, 7), 0xffff);
    }

    #[test]
    fn linha_fora_da_tela_nao_estoura() {
        let mut fb = Framebuffer::new(4, 4);
        fb.draw_line(-100, -100, 100, 100, Rgb::WHITE);
        assert_eq!(fb.get_pixel(0, 0), 0xffff);
    }

    #[test]
    fn circulo_toca_os_quatro_extremos() {
        let mut fb = Framebuffer::new(16, 16);
        fb.draw_circle(8, 8, 4, Rgb::WHITE);
        assert_eq!(fb.get_pixel(12, 8), 0xffff);
        assert_eq!(fb.get_pixel(4, 8), 0xffff);
        assert_eq!(fb.get_pixel(8, 12), 0xffff);
        assert_eq!(fb.get_pixel(8, 4), 0xffff);
        assert_eq!(fb.get_pixel(8, 8), 0, "o miolo fica vazio: é só a borda");
    }

    #[test]
    fn circulo_preenchido_pinta_o_centro() {
        let mut fb = Framebuffer::new(16, 16);
        fb.fill_circle(8, 8, 4, Rgb::WHITE);
        assert_eq!(fb.get_pixel(8, 8), 0xffff);
        assert_eq!(fb.get_pixel(8, 15), 0, "fora do raio continua vazio");
    }

    #[test]
    fn poligono_preenchido_cobre_o_interior() {
        let mut fb = Framebuffer::new(10, 10);
        // Um quadrado de (2,2) a (7,7).
        fb.fill_polygon(&[(2, 2), (7, 2), (7, 7), (2, 7)], Rgb::WHITE);
        assert_eq!(fb.get_pixel(4, 4), 0xffff);
        assert_eq!(fb.get_pixel(2, 2), 0xffff);
        assert_eq!(fb.get_pixel(1, 1), 0, "fora do polígono continua vazio");
    }

    #[test]
    fn blit_copia_regiao_e_respeita_transparencia() {
        let mut src = Framebuffer::new(2, 2);
        src.set_pixel_native(0, 0, 0x1234);
        src.set_pixel_native(1, 0, 0xf81f); // cor marcada como transparente
        src.set_pixel_native(0, 1, 0x4321);

        let mut dst = Framebuffer::new(2, 2);
        dst.blit(0, 0, 2, 2, &src, 0, 0, Some(0xf81f));
        assert_eq!(dst.get_pixel(0, 0), 0x1234);
        assert_eq!(
            dst.get_pixel(1, 0),
            0,
            "pixel transparente não deve ser copiado"
        );
        assert_eq!(dst.get_pixel(0, 1), 0x4321);
    }

    #[test]
    fn blit_recorta_nas_bordas_das_duas_superficies() {
        let mut src = Framebuffer::new(2, 2);
        src.fill_rect_native(
            Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            0xffff,
        );
        let mut dst = Framebuffer::new(2, 2);
        // Origem e destino fora dos limites: não deve estourar nem escrever nada.
        dst.blit(-5, -5, 2, 2, &src, 10, 10, None);
        assert!(!dst.is_dirty());
    }

    #[test]
    fn bmp_tem_cabecalho_e_tamanho_certos() {
        let mut fb = Framebuffer::new(2, 2);
        fb.set_pixel(0, 0, Rgb::WHITE);
        let bmp = fb.to_bmp();
        assert_eq!(&bmp[0..2], b"BM");
        // 2 pixels por linha = 6 bytes, arredondado para 8; duas linhas = 16.
        assert_eq!(bmp.len(), 54 + 16);
        assert_eq!(
            u32::from_le_bytes([bmp[2], bmp[3], bmp[4], bmp[5]]),
            bmp.len() as u32
        );
    }
}
