//! OpenGL ES 1.1: o despacho das chamadas e a ponte para o rasterizador.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Um ajuste de textura vindo da extensão, com os nomes do OpenGL ES.
    pub(super) fn apply_texture_setting(&mut self, name: &str, pname: u32, value: u32) {
        match name.starts_with("TexEnv") {
            true if pname == gles::GL_TEXTURE_ENV_MODE => self.gl.set_texture_env(value),
            true => {}
            false => self.gl.set_texture_parameter(pname, value),
        }
    }

    /// As chamadas de GL que atendemos sem fazer nada.
    pub fn ignored_gl(&self) -> Vec<&'static str> {
        self.ignored_gl.iter().copied().collect()
    }

    /// O OpenGL ES, nas duas formas em que o BREW o expõe.
    ///
    /// A nova é o `IGLES11` de `sdk/inc/AEEGLES10.h` e `AEEGLES11.h`; a antiga é o `IGL` de
    /// `sdk/inc/AEEGL.h`, que não recebe o `this` e devolve o resultado direto. Como no EGL,
    /// tirando o prefixo `gl` os nomes coincidem, e um tradutor só atende as duas.
    ///
    /// Aqui não se desenha nada: os argumentos viram estado ou vértices, e quem rasteriza é o
    /// [`rasterizer`].
    pub(super) fn gles_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(full) = iface.method(slot) else {
            return Ok(None);
        };
        let legacy = iface == Interface::GlLegacy;
        let name = full.strip_prefix("gl").unwrap_or(full);
        if name.starts_with("Draw") || matches!(name, "Clear" | "ReadPixels") {
            self.sync_egl_color_from_guest()?;
        }
        let base = usize::from(!legacy);
        let a: [u32; 10] = std::array::from_fn(|i| self.arg(base + i));
        let this = self.arg(0);
        // As variantes `x` levam ponto fixo 16.16 e as `f`, `float` de 32 bits — mesma função,
        // só muda como o número chega.
        let fixed = |v: u32| gles::fixed(v);
        let float = |v: u32| f32::from_bits(v);
        let number = |v: u32| {
            if name.ends_with('x') {
                fixed(v)
            } else {
                float(v)
            }
        };
        // Os poucos métodos que produzem um valor: na forma antiga ele é o retorno, na nova
        // vai para um ponteiro de saída.
        let mut answer = None;
        match name {
            "AddRef" => return Ok(Some(self.objects.add_ref(this))),
            "Release" => return Ok(Some(self.objects.release(this))),
            "QueryInterface" => {
                self.write_at(a[1], this)?;
                return Ok(Some(SUCCESS));
            }
            "GetError" => answer = Some((0, gles::GL_NO_ERROR)),
            "GetString" => {
                let text = match a[0] {
                    gles::GL_VENDOR => "Zeebx",
                    gles::GL_RENDERER => "Zeebx Software Rasterizer",
                    gles::GL_VERSION => "OpenGL ES-CM 1.1",
                    // Só o que existe de verdade. Anunciar extensão que não temos faria o jogo
                    // chamar função que não existe — e omitir uma que temos é pior ainda: os
                    // dez portes de arcade do console conferem o `GL_OES_draw_texture` aqui e
                    // desistem da inicialização gráfica sem ele.
                    // O `atitc` entrou porque nós o decodificamos de verdade — ver `atc.rs` e
                    // o `gles_compressed_tex_image`. A Z-Wheel procura por ele antes de montar
                    // o palco, cujas texturas (`stage_*.qxt`) são ATITC; sem o nome na lista ela
                    // desiste do palco inteiro.
                    //
                    // Os `vertex_buffer_object` e o `point_size_array` que ela também procura
                    // ficam **de fora**: do primeiro não temos `BindBuffer` nem `BufferData`, e
                    // do segundo só um `SUCCESS` que não faz nada. Anunciar o que não existe faz
                    // o jogo chamar função que não está lá.
                    gles::GL_EXTENSIONS => {
                        "GL_OES_draw_texture GL_ATI_imageon_misc GL_ATI_texture_compression_atitc "
                    }
                    _ => "",
                };
                let addr = self.intern(text)?;
                answer = Some((1, addr));
            }
            // A spec só exige nomes distintos e fora de uso, então uma sequência serve.
            "GenTextures" | "GenBuffers" => {
                let (count, out) = (a[0], a[1]);
                for i in 0..count {
                    self.gles_next_name += 1;
                    if out != 0 {
                        self.cpu.write_u32(out + i * 4, self.gles_next_name)?;
                    }
                }
            }
            "DeleteTextures" => {
                for i in 0..a[0] {
                    let name = self.cpu.read_u32(a[1] + i * 4)?;
                    // A fila menciona texturas pelo nome; apagar uma antes de ser lida
                    // mudaria o que já foi desenhado.
                    self.gl.flush();
                    self.gl.textures.remove(&name);
                }
            }

            // --- Matrizes ---------------------------------------------------------------
            "MatrixMode" => self.gl.set_matrix_mode(a[0]),
            "LoadIdentity" => self.gl.load_identity(),
            "PushMatrix" => self.gl.push_matrix(),
            "PopMatrix" => self.gl.pop_matrix(),
            "LoadMatrixx" | "LoadMatrixf" | "MultMatrixx" | "MultMatrixf" => {
                let m = self.read_matrix(a[0], name.ends_with('x'))?;
                if name.starts_with("Load") {
                    self.gl.load_matrix(m);
                } else {
                    self.gl.mult_matrix(m);
                }
            }
            "Translatex" | "Translatef" => {
                let m = rasterizer::translation(number(a[0]), number(a[1]), number(a[2]));
                self.gl.mult_matrix(m);
            }
            "Scalex" | "Scalef" => {
                let m = rasterizer::scaling(number(a[0]), number(a[1]), number(a[2]));
                self.gl.mult_matrix(m);
            }
            "Rotatex" | "Rotatef" => {
                let m =
                    rasterizer::rotation(number(a[0]), number(a[1]), number(a[2]), number(a[3]));
                self.gl.mult_matrix(m);
            }
            "Frustumx" | "Frustumf" | "Orthox" | "Orthof" => {
                let v: Vec<f32> = a[..6].iter().map(|&word| number(word)).collect();
                let m = if name.starts_with("Frustum") {
                    rasterizer::frustum(v[0], v[1], v[2], v[3], v[4], v[5])
                } else {
                    rasterizer::ortho(v[0], v[1], v[2], v[3], v[4], v[5])
                };
                self.gl.mult_matrix(m);
            }

            // --- Estado -----------------------------------------------------------------
            "Viewport" => self
                .gl
                .set_viewport(a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32),
            // --- Stencil ------------------------------------------------------------
            //
            // O palco da Z-Wheel arma estes duas vezes por quadro: é o reflexo plano, que marca
            // o chão no stencil e desenha o modelo espelhado só onde a marca ficou.
            "StencilFunc" => self.gl.set_stencil_func(a[0], a[1] as i32, a[2]),
            "StencilOp" => self.gl.set_stencil_op(a[0], a[1], a[2]),
            "StencilMask" => self.gl.set_stencil_mask(a[0]),
            "ClearStencil" => self.gl.set_clear_stencil(a[0] as i32),
            "Clear" => {
                if a[0] & gles::GL_COLOR_BUFFER_BIT != 0 {
                    self.gl_clears = self.gl_clears.saturating_add(1);
                }
                self.gl.clear(a[0])
            }
            "ClearColorx" | "ClearColor" => {
                let c = std::array::from_fn(|i| number(a[i]));
                self.gl.set_clear_color(c);
            }
            // `ClearDepth` já recebe a profundidade em `[0, 1]`, que é a faixa do buffer.
            "ClearDepthx" | "ClearDepthf" => self.gl.set_clear_depth(number(a[0]).clamp(0.0, 1.0)),
            "Color4x" | "Color4f" => {
                let c = std::array::from_fn(|i| number(a[i]));
                self.gl.set_color(c);
            }
            "Color4ub" => {
                let c = std::array::from_fn(|i| (a[i] & 0xff) as f32 / 255.0);
                self.gl.set_color(c);
            }
            "Enable" => self.gl.set_capability(a[0], true),
            "Disable" => self.gl.set_capability(a[0], false),
            "BlendFunc" => self.gl.set_blend_func(a[0], a[1]),
            "DepthFunc" => self.gl.set_depth_func(a[0]),
            "DepthMask" => self.gl.set_depth_mask(a[0] != 0),
            "AlphaFuncx" | "AlphaFunc" => self.gl.set_alpha_func(a[0], number(a[1])),
            "CullFace" => self.gl.set_cull_face(a[0]),
            "FrontFace" => self.gl.set_front_face(a[0]),
            "TexParameterx" | "TexParameteri" | "TexParameterf" => {
                self.gl.set_texture_parameter(a[1], a[2])
            }
            // As formas vetoriais trazem o valor por ponteiro. O Crash pede o `GL_REPLACE`
            // por aqui, e enquanto só a forma escalar era atendida o modo ficava preso no
            // `GL_MODULATE`: cada textura saía multiplicada pela cor do vértice.
            "TexParameterxv" | "TexParameteriv" | "TexParameterfv" => {
                match a[1] == gles::GL_TEXTURE_CROP_RECT_OES {
                    // O recorte são quatro inteiros com sinal, e o sinal importa: largura ou
                    // altura negativa espelha o eixo.
                    true => {
                        let mut crop = [0i32; 4];
                        for (index, slot) in crop.iter_mut().enumerate() {
                            *slot = self.cpu.read_u32(a[2] + index as u32 * 4)? as i32;
                        }
                        self.gl.set_texture_crop(crop);
                    }
                    false => {
                        let value = self.cpu.read_u32(a[2])?;
                        self.gl.set_texture_parameter(a[1], value);
                    }
                }
            }
            // void glDrawTex{sixf}OES(T x, T y, T z, T width, T height) e as formas vetoriais,
            // que trazem os cinco valores por ponteiro.
            //
            // `s` é inteiro de 16 bits, `i` de 32, `x` é ponto fixo 16.16 e `f` é float. Todas
            // desenham a mesma coisa; só muda como o número chega.
            name if name.starts_with("DrawTex") => {
                let vector = name.ends_with("vOES");
                let scale = match name.as_bytes().get(7) {
                    Some(b'x') => 1.0 / 65536.0,
                    _ => 1.0,
                };
                let float = name.as_bytes().get(7) == Some(&b'f');
                let mut values = [0f32; 5];
                for (index, slot) in values.iter_mut().enumerate() {
                    let raw = match vector {
                        true => self.cpu.read_u32(a[0] + index as u32 * 4)?,
                        false => self.arg(index + 1),
                    };
                    *slot = match float {
                        true => f32::from_bits(raw),
                        false => raw as i32 as f32 * scale,
                    };
                }
                let [x, y, z, width, height] = values;
                self.gl.draw_texture(x, y, z, width, height);
            }
            "TexEnvx" | "TexEnvi" | "TexEnvf" => {
                if a[1] == gles::GL_TEXTURE_ENV_MODE {
                    // O modo é um enum, mesmo quando chega pela variante de ponto fixo.
                    self.gl.set_texture_env(a[2]);
                }
            }
            "TexEnvxv" | "TexEnviv" | "TexEnvfv" => {
                if a[1] == gles::GL_TEXTURE_ENV_MODE {
                    let value = self.cpu.read_u32(a[2])?;
                    self.gl.set_texture_env(value);
                }
            }
            "ActiveTexture" => self.gl.set_active_texture(a[0]),
            "ClientActiveTexture" => self.gl.set_client_active_texture(a[0]),
            "BindTexture" => self.gl.bind_texture(a[1]),
            "TexImage2D" => self.gles_tex_image(&a)?,
            "TexSubImage2D" => self.gles_tex_sub_image(&a)?,
            "CompressedTexImage2D" => self.gles_compressed_tex_image(&a)?,

            // --- Iluminação de função fixa ---------------------------------------------
            //
            // O palco da Z-Wheel depende dela: liga `GL_LIGHTING` e `GL_LIGHT0`, põe a ambiente
            // da luz em 0,5 e o material ambiente e difuso em 0,949, e deixa todo o resto no
            // padrão — inclusive a difusa branca da luz zero e a posição `(0, 0, 1, 0)`, que é
            // direcional. Sem nada disso, os modelos saíam com a cor de vértice crua.
            "Lightxv" | "Lightfv" => {
                let luz = a[0].wrapping_sub(gles::GL_LIGHT0) as usize;
                let valores = self.le_parametro(a[1], a[2], name.ends_with("xv"))?;
                self.gl.set_light(luz, a[1], valores);
            }
            "Materialxv" | "Materialfv" => {
                let valores = self.le_parametro(a[1], a[2], name.ends_with("xv"))?;
                self.gl.set_material(a[1], valores);
            }
            "LightModelxv" | "LightModelfv" => {
                let valores = self.le_parametro(a[0], a[1], name.ends_with("xv"))?;
                self.gl.set_light_model(a[0], valores);
            }
            // As formas escalares trazem o valor no próprio argumento.
            "Lightx" | "Lightf" => {
                let luz = a[0].wrapping_sub(gles::GL_LIGHT0) as usize;
                let valor = escalar(a[2], name.ends_with('x'));
                self.gl.set_light(luz, a[1], [valor, 0.0, 0.0, 0.0]);
            }
            "Materialx" | "Materialf" => {
                let valor = escalar(a[2], name.ends_with('x'));
                self.gl.set_material(a[1], [valor, 0.0, 0.0, 0.0]);
            }
            "LightModelx" | "LightModelf" => {
                let valor = escalar(a[1], name.ends_with('x'));
                self.gl.set_light_model(a[0], [valor, 0.0, 0.0, 0.0]);
            }
            "ShadeModel" => self.gl.set_shade_model(a[0]),
            "Normal3x" | "Normal3f" => {
                let fixo = name.ends_with('x');
                self.gl_normal_atual = std::array::from_fn(|i| escalar(a[i], fixo));
            }
            // glNormalPointer(type, stride, pointer) — **sem tamanho**: normal é sempre de três.
            "NormalPointer" => {
                let enabled = self.gl_normals.enabled;
                self.gl_normals = ArrayPointer {
                    size: 3,
                    kind: a[0],
                    stride: a[1],
                    address: a[2],
                    enabled,
                };
            }

            // --- Vetores e desenho ------------------------------------------------------
            "VertexPointer" | "ColorPointer" | "TexCoordPointer" => {
                let pointer = ArrayPointer {
                    size: a[0],
                    kind: a[1],
                    stride: a[2],
                    address: a[3],
                    enabled: true,
                };
                // O vetor de coordenadas pertence à unidade escolhida pelo
                // `glClientActiveTexture`; as outras unidades não têm onde cair aqui.
                if name == "TexCoordPointer" && !self.gl.base_client_unit() {
                    return Ok(Some(SUCCESS));
                }
                let slot = match name {
                    "VertexPointer" => &mut self.gl_vertices,
                    "ColorPointer" => &mut self.gl_colors,
                    _ => &mut self.gl_texcoords,
                };
                // `enabled` é do `EnableClientState`, não do ponteiro: trocar o ponteiro não
                // liga nem desliga o vetor.
                let enabled = slot.enabled;
                *slot = ArrayPointer { enabled, ..pointer };
            }
            "EnableClientState" | "DisableClientState" => {
                let on = name.starts_with("Enable");
                match a[0] {
                    gles::GL_VERTEX_ARRAY => self.gl_vertices.enabled = on,
                    gles::GL_COLOR_ARRAY => self.gl_colors.enabled = on,
                    gles::GL_NORMAL_ARRAY => self.gl_normals.enabled = on,
                    gles::GL_TEXTURE_COORD_ARRAY if self.gl.base_client_unit() => {
                        self.gl_texcoords.enabled = on
                    }
                    gles::GL_TEXTURE_COORD_ARRAY => {}
                    _ => {}
                }
            }
            "DrawArrays" => {
                let indices: Vec<u32> = (0..a[2]).map(|i| a[1] + i).collect();
                self.gles_draw(a[0], &indices)?;
            }
            "DrawElements" => {
                let (mode, count, kind, list) = (a[0], a[1], a[2], a[3]);
                let mut indices = Vec::with_capacity(count as usize);
                for i in 0..count {
                    indices.push(match kind {
                        gles::GL_UNSIGNED_BYTE => {
                            let mut byte = [0u8; 1];
                            self.cpu.read_mem(list + i, &mut byte)?;
                            byte[0] as u32
                        }
                        _ => {
                            let mut half = [0u8; 2];
                            self.cpu.read_mem(list + i * 2, &mut half)?;
                            u16::from_le_bytes(half) as u32
                        }
                    });
                }
                self.gles_draw(mode, &indices)?;
            }

            "GetIntegerv" | "GetFixedv" | "GetBooleanv" => {
                let (width, height) = (self.gl.width as i32, self.gl.height as i32);
                let values = gles::integer(a[0], width, height).unwrap_or(&[0]);
                for (i, &value) in values.iter().enumerate() {
                    if a[1] != 0 {
                        self.cpu.write_u32(a[1] + i as u32 * 4, value as u32)?;
                    }
                }
            }
            // Os outros `Get*v` escrevem no ponteiro do segundo argumento; zerar é melhor que
            // deixar lixo, e nenhum jogo depende deles ainda.
            name if name.starts_with("Get") => self.write_at(a[1], 0)?,
            // O resto é atendido com sucesso e não faz nada. Isso é deliberado para o estado que
            // o nosso rasterizador não usa — profundidade, névoa, luz —, e recusar derrubaria
            // jogos por nada. Mas o silêncio esconde as que **mudam o desenho**: o
            // `TexSubImage2D` estava aqui, e o efeito era textura embaralhada sem uma linha de
            // aviso. Registrar não custa, e dá por onde começar a investigar um desenho errado.
            // glReadPixels(x, y, width, height, format, type, pixels)
            //
            // É como o jogo faz a foto do boneco: desenha e lê o quadro de volta. Enquanto isto
            // não existia, ele lia o que estivesse no buffer dele — daí a imagem embaralhada.
            "ReadPixels" => self.gles_read_pixels(&a)?,
            // glColorMask(r, g, b, a) — booleanos, um por canal.
            "ColorMask" => self.gl.set_color_mask(std::array::from_fn(|i| a[i] != 0)),
            outro => {
                if !ATENDIDAS_EM_SILENCIO.contains(&outro) {
                    self.ignored_gl.insert(full);
                }
            }
        }
        match answer {
            Some((_, value)) if legacy => Ok(Some(value)),
            Some((out, value)) => {
                self.write_at(a[out], value)?;
                Ok(Some(SUCCESS))
            }
            None => Ok(Some(SUCCESS)),
        }
    }

    /// Lê os dezesseis números de uma matriz da memória do guest.
    pub(super) fn read_matrix(
        &self,
        address: u32,
        fixed_point: bool,
    ) -> Result<rasterizer::Matrix, CpuError> {
        let mut m = rasterizer::IDENTITY;
        for (i, slot) in m.iter_mut().enumerate() {
            let word = self.cpu.read_u32(address + i as u32 * 4)?;
            *slot = if fixed_point {
                gles::fixed(word)
            } else {
                f32::from_bits(word)
            };
        }
        Ok(m)
    }

    /// `TexImage2D(target, level, internalformat, width, height, border, format, type,
    /// pixels)` — nove argumentos, dos quais a maioria chega pela pilha.
    /// `glCompressedTexImage2D(target, level, internalformat, width, height, border,
    /// imageSize, data)`.
    ///
    /// Só os formatos ATITC, que são os do Adreno 130 e os únicos que os jogos do console usam
    /// — o Boomerang Sports Dodgeball carrega todas as texturas dele assim.
    pub(super) fn gles_compressed_tex_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, format, width, height) = (a[1], a[2], a[3], a[4]);
        let (size, pixels) = (a[6], a[7]);
        // As texturas paletizadas do OES entram pelo mesmo caminho. O `level` delas é não
        // positivo — zero é só o nível base, e um negativo diz quantos mipmaps vêm depois —,
        // então a checagem de nível abaixo não vale para elas.
        if let Some(palette) = paltex::Format::from_gl(format) {
            if width == 0 || height == 0 || pixels == 0 {
                return Ok(());
            }
            let bytes = self.read_bytes(pixels, size)?;
            let Some(decoded) = paltex::decode(&bytes, width as usize, height as usize, palette)
            else {
                self.bad_pointers.insert(format!(
                    "textura paletizada {format:#x} sem paleta completa"
                ));
                return Ok(());
            };
            let name = self.gl.bound_texture();
            // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
            self.gl.flush();
            let texture = self.gl.textures.entry(name).or_default();
            // A paletizada traz a cadeia inteira num bloco só, e o `level` dela conta os
            // mipmaps em vez de nomeá-los; o decodificador devolve o nível base.
            guarda_nivel(texture, 0, width as usize, height as usize, decoded);
            return Ok(());
        }
        let explicit_alpha = match format {
            gles::GL_ATC_RGB_AMD => false,
            gles::GL_ATC_RGBA_EXPLICIT_ALPHA_AMD => true,
            _ => {
                self.bad_pointers
                    .insert(format!("textura comprimida no formato {format:#x}"));
                return Ok(());
            }
        };
        if width == 0 || height == 0 || pixels == 0 {
            return Ok(());
        }
        let bytes = self.read_bytes(pixels, size)?;
        let decoded = atc::decode(&bytes, width as usize, height as usize, explicit_alpha);

        let name = self.gl.bound_texture();
        // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
        self.gl.flush();
        let texture = self.gl.textures.entry(name).or_default();
        guarda_nivel(texture, level, width as usize, height as usize, decoded);
        Ok(())
    }

    pub(super) fn gles_tex_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, width, height) = (a[1], a[3], a[4]);
        let (format, kind, pixels) = (a[6], a[7], a[8]);
        if width == 0 || height == 0 {
            return Ok(());
        }
        let texels = (width * height) as usize;
        let decoded = if pixels == 0 {
            vec![[255; 4]; texels]
        } else {
            let bytes = self.read_bytes(pixels, width * height * bytes_per_texel(format, kind))?;
            decode_texels(&bytes, format, kind, texels)
        };
        // Os parâmetros de repetição e filtro sobrevivem a uma nova imagem: no OpenGL eles são
        // do nome da textura, não do conteúdo, e o jogo costuma defini-los uma vez só.
        let name = self.gl.bound_texture();
        // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
        self.gl.flush();
        let texture = self.gl.textures.entry(name).or_default();
        guarda_nivel(texture, level, width as usize, height as usize, decoded);
        Ok(())
    }

    /// `glReadPixels`: copia um retângulo do quadro para a memória do jogo.
    ///
    /// Atendemos os dois formatos que o OpenGL ES 1.1 obriga: `RGBA` de oito bits por canal e
    /// `RGB` em 565. Qualquer outro é registrado em vez de escrito, porque preencher com o
    /// formato errado dá uma imagem plausível e falsa — pior que não escrever.
    pub(super) fn gles_read_pixels(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (x, y) = (a[0] as i32, a[1] as i32);
        let (width, height) = (a[2] as usize, a[3] as usize);
        let (format, kind, destino) = (a[4], a[5], a[6]);
        if width == 0 || height == 0 || destino == 0 {
            return Ok(());
        }
        let pixels = self.gl.read_rect(x, y, width, height);
        let bytes: Vec<u8> = match (format, kind) {
            (gles::GL_RGBA, gles::GL_UNSIGNED_BYTE) => pixels.concat(),
            (gles::GL_RGB, gles::GL_UNSIGNED_SHORT_5_6_5) => pixels
                .iter()
                .flat_map(|p| {
                    let v = (u16::from(p[0] >> 3) << 11)
                        | (u16::from(p[1] >> 2) << 5)
                        | u16::from(p[2] >> 3);
                    v.to_le_bytes()
                })
                .collect(),
            _ => {
                self.bad_pointers.insert(format!(
                    "ReadPixels no formato {format:#x}/{kind:#x}, que não sabemos escrever"
                ));
                return Ok(());
            }
        };
        self.cpu.write_mem(destino, &bytes)?;
        Ok(())
    }

    /// `glTexSubImage2D`: troca um retângulo de dentro de uma textura que já existe.
    ///
    /// É como se monta imagem em pedaços — um retrato dentro de um atlas, um número que muda —,
    /// e enquanto isto não existia a chamada era atendida em silêncio: a textura ficava com o
    /// conteúdo antigo e o desenho saía embaralhado.
    ///
    /// Se o retângulo não couber na textura, não escrevemos nada. Recortar seria inventar um
    /// resultado que o OpenGL não define.
    pub(super) fn gles_tex_sub_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, x, y, width, height) = (a[1], a[2], a[3], a[4], a[5]);
        let (format, kind, pixels) = (a[6], a[7], a[8]);
        if level != 0 || width == 0 || height == 0 || pixels == 0 {
            return Ok(());
        }
        let texels = (width * height) as usize;
        let bytes = self.read_bytes(pixels, width * height * bytes_per_texel(format, kind))?;
        let novos = decode_texels(&bytes, format, kind, texels);

        let name = self.gl.bound_texture();
        // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
        self.gl.flush();
        let Some(texture) = self.gl.textures.get_mut(&name) else {
            return Ok(());
        };
        let (tw, th) = (texture.width as u32, texture.height as u32);
        if x + width > tw || y + height > th {
            self.bad_pointers.insert(format!(
                "TexSubImage2D de {width}x{height} em ({x},{y}) não cabe numa textura {tw}x{th}"
            ));
            return Ok(());
        }
        for linha in 0..height {
            let destino = ((y + linha) * tw + x) as usize;
            let origem = (linha * width) as usize;
            texture.pixels[destino..destino + width as usize]
                .copy_from_slice(&novos[origem..origem + width as usize]);
        }
        // **Mexer no nível zero invalida a cadeia.** Os níveis menores continuariam mostrando o
        // que estava ali antes, e quem amostra dois níveis vê os dois conteúdos ao mesmo tempo:
        // o painel de promoção da Z-Wheel, que troca o texto por aqui, saía com as letras
        // fantasmas do texto anterior por cima das novas.
        texture.mipmaps.clear();
        Ok(())
    }

    /// Monta os vértices a partir dos vetores do cliente e manda desenhar.
    pub(super) fn gles_draw(&mut self, mode: u32, indices: &[u32]) -> Result<(), CpuError> {
        if !self.gl_vertices.enabled || self.gl_vertices.address == 0 || indices.is_empty() {
            return Ok(());
        }
        let base = self.gl.current_color();
        let mut vertices = Vec::with_capacity(indices.len());
        for &index in indices {
            let position = self.read_attribute(self.gl_vertices, index, [0.0, 0.0, 0.0, 1.0])?;
            let color = if self.gl_colors.enabled && self.gl_colors.address != 0 {
                self.read_attribute(self.gl_colors, index, [0.0, 0.0, 0.0, 1.0])?
            } else {
                base
            };
            let uv = if self.gl_texcoords.enabled && self.gl_texcoords.address != 0 {
                let t = self.read_attribute(self.gl_texcoords, index, [0.0; 4])?;
                [t[0], t[1]]
            } else {
                [0.0; 2]
            };
            let normal = if self.gl_normals.enabled && self.gl_normals.address != 0 {
                let n = self.read_attribute(self.gl_normals, index, [0.0, 0.0, 1.0, 0.0])?;
                [n[0], n[1], n[2]]
            } else {
                self.gl_normal_atual
            };
            vertices.push(Vertex {
                position,
                color,
                uv,
                normal,
            });
        }
        self.gl.draw(mode, &vertices);
        Ok(())
    }

    /// Lê os componentes de um parâmetro de luz ou material do ponteiro do jogo.
    ///
    /// Quantos ler vem do próprio parâmetro — ver [`gles::componentes`] —, e não quatro sempre:
    /// o `GL_SHININESS` tem um só, e ler quatro passa por cima do que estiver depois dele na
    /// pilha do jogo.
    pub(super) fn le_parametro(
        &self,
        pname: u32,
        ponteiro: u32,
        fixo: bool,
    ) -> Result<[f32; 4], CpuError> {
        let mut valores = [0.0f32; 4];
        for (i, valor) in valores
            .iter_mut()
            .enumerate()
            .take(gles::componentes(pname))
        {
            *valor = escalar(self.cpu.read_u32(ponteiro + i as u32 * 4)?, fixo);
        }
        Ok(valores)
    }

    /// Lê um elemento de um vetor do cliente, completando os componentes que faltam.
    pub(super) fn read_attribute(
        &self,
        pointer: ArrayPointer,
        index: u32,
        default: [f32; 4],
    ) -> Result<[f32; 4], CpuError> {
        let component = component_size(pointer.kind);
        let stride = if pointer.stride == 0 {
            pointer.size * component
        } else {
            pointer.stride
        };
        let base = pointer.address + index * stride;
        let mut out = default;
        for i in 0..pointer.size.min(4) {
            let address = base + i * component;
            out[i as usize] = match pointer.kind {
                gles::GL_FLOAT => f32::from_bits(self.cpu.read_u32(address)?),
                gles::GL_FIXED => gles::fixed(self.cpu.read_u32(address)?),
                gles::GL_SHORT => {
                    let mut half = [0u8; 2];
                    self.cpu.read_mem(address, &mut half)?;
                    i16::from_le_bytes(half) as f32
                }
                gles::GL_UNSIGNED_SHORT => {
                    let mut half = [0u8; 2];
                    self.cpu.read_mem(address, &mut half)?;
                    u16::from_le_bytes(half) as f32
                }
                gles::GL_BYTE => {
                    let mut byte = [0u8; 1];
                    self.cpu.read_mem(address, &mut byte)?;
                    byte[0] as i8 as f32
                }
                // `GL_UNSIGNED_BYTE` só aparece em cor, e ali o valor é normalizado.
                _ => {
                    let mut byte = [0u8; 1];
                    self.cpu.read_mem(address, &mut byte)?;
                    byte[0] as f32 / 255.0
                }
            };
        }
        Ok(out)
    }

    /// N-ésimo argumento da AAPCS: `r0..r3` e, daí em diante, palavras da pilha.
    pub(super) fn arg(&self, index: usize) -> u32 {
        match index {
            0 => self.cpu.read_reg(Reg::R0),
            1 => self.cpu.read_reg(Reg::R1),
            2 => self.cpu.read_reg(Reg::R2),
            3 => self.cpu.read_reg(Reg::R3),
            n => {
                let sp = self.cpu.read_reg(Reg::Sp);
                self.cpu.read_u32(sp + (n as u32 - 4) * 4).unwrap_or(0)
            }
        }
    }

    /// Copia o quadro do OpenGL para a tela.
    ///
    /// É o que o `eglSwapBuffers` faz no console: o buffer de trás vira o da frente. Aqui a
    /// tela é o framebuffer RGB565 que já sabemos exportar.
    pub(super) fn present_gl(&mut self) {
        // A tela pode ser a superfície do "device bitmap", quando o jogo pediu uma — é ela que
        // vale, e não o framebuffer de reserva.
        let (width, height) = {
            let target = self.screen();
            (target.width() as usize, target.height() as usize)
        };
        let frame = self.gl.present(width, height);
        let bytes: Vec<u8> = frame.iter().flat_map(|p| p.to_le_bytes()).collect();
        match self.bitmaps.get_mut(&self.device_bitmap) {
            Some(surface) => surface.load_rgb565_bytes(&bytes),
            None => self.screen.load_rgb565_bytes(&bytes),
        }
        self.gl_last_frame = bytes;
    }
}
