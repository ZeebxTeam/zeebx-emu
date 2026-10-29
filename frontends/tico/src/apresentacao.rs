//! O quadro RGB565 do console dentro do framebuffer do Switch.
//!
//! A tela portátil é 1280×720 e a dock em 1080p é 1920×1080. O Zeebo entrega 640×480. Esticar
//! até encher com o vizinho mais próximo repete uma coluna e não a seguinte — foi o que a Vita
//! mediu, e a imagem treme. A escala inteira que cabe (1× na portátil, 2× na dock 1080p) só
//! duplica pixel, e a faixa fica preta.

/// A maior escala inteira que cabe. Zero quando a origem é maior que o destino.
pub fn escala_inteira(dest_w: usize, dest_h: usize, origem_w: usize, origem_h: usize) -> usize {
    if origem_w == 0 || origem_h == 0 {
        return 0;
    }
    (dest_w / origem_w).min(dest_h / origem_h)
}

/// Copia `origem` para `destino`. `passo_bytes` é a distância entre linhas, em bytes.
///
/// O buffer inteiro é apagado antes: o framebuffer volta sujo, e a faixa de uma escala menor
/// não pode ficar com o quadro anterior.
pub fn copia_rgb565(
    destino: &mut [u16],
    passo_bytes: usize,
    dest_w: usize,
    dest_h: usize,
    origem: &[u16],
    origem_w: usize,
    origem_h: usize,
) {
    if passo_bytes < 2 || dest_w == 0 || dest_h == 0 {
        return;
    }
    let passo = passo_bytes / 2;
    let preciso = passo.saturating_mul(dest_h);
    if preciso == 0 || destino.len() < preciso {
        return;
    }
    destino[..preciso].fill(0);
    if origem_w == 0
        || origem_h == 0
        || origem.len() < origem_w.saturating_mul(origem_h)
        || dest_w > passo
    {
        return;
    }
    let escala = escala_inteira(dest_w, dest_h, origem_w, origem_h);
    if escala >= 1 {
        let w = origem_w * escala;
        let h = origem_h * escala;
        let x0 = (dest_w - w) / 2;
        let y0 = (dest_h - h) / 2;
        for y in 0..h {
            let sy = y / escala;
            let linha = (y0 + y) * passo + x0;
            let src = sy * origem_w;
            for x in 0..w {
                destino[linha + x] = origem[src + x / escala];
            }
        }
        return;
    }
    // A origem não cabe nem em 1×. O vizinho mais próximo é o que resta; a tela do
    // Switch é maior que 640×480, então isto só aparece num framebuffer pequeno de teste.
    for y in 0..dest_h {
        let sy = y * origem_h / dest_h;
        let linha = y * passo;
        let src = sy * origem_w;
        for x in 0..dest_w {
            let sx = x * origem_w / dest_w;
            destino[linha + x] = origem[src + sx];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portatil_fica_em_1x_e_a_dock_1080p_em_2x() {
        assert_eq!(escala_inteira(1280, 720, 640, 480), 1);
        assert_eq!(escala_inteira(1920, 1080, 640, 480), 2);
    }

    #[test]
    fn a_escala_inteira_duplica_o_pixel_e_apaga_a_faixa() {
        // 2×2 numa tela de 8×5: 3× já não cabe na altura, então a escala é 2.
        let origem = [0xF800u16, 0x07E0, 0x001F, 0xFFFF];
        let mut destino = vec![0x1234u16; 8 * 5];
        copia_rgb565(&mut destino, 8 * 2, 8, 5, &origem, 2, 2);
        assert_eq!(escala_inteira(8, 5, 2, 2), 2);
        // Bloco de 4×4 encostado em cima, centrado: x começa em 2.
        assert_eq!(destino[0], 0, "a coluna da esquerda fica preta");
        assert_eq!(destino[2], 0xF800);
        assert_eq!(destino[3], 0xF800);
        assert_eq!(destino[4], 0x07E0);
        assert_eq!(destino[2 * 8 + 2], 0x001F);
        assert_eq!(destino[4 * 8], 0, "a faixa de baixo fica preta");
    }

    #[test]
    fn um_passo_curto_demais_nao_escreve() {
        let mut destino = [7u16; 4];
        copia_rgb565(&mut destino, 1, 2, 2, &[1, 2, 3, 4], 2, 2);
        assert_eq!(destino, [7, 7, 7, 7]);
    }
}
