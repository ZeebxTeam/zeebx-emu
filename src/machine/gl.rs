//! OpenGL ES 1.1: o despacho das chamadas e a ponte para o rasterizador.

use super::*;

/// Teto do trecho que a leitura em bloco aceita montar de uma vez. Um ponteiro de array
/// corrompido dá uma extensão absurda, e alocar isso seria trocar uma lentidão por um estouro
/// de memória. Acima daqui, volta a ler componente a componente — que é lento, mas seguro.
const TETO_DO_ARRAY: u64 = 8 << 20;

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
                    // O `vertex_buffer_object` entrou quando passou a existir: `BindBuffer`,
                    // `BufferData`, `BufferSubData`, `DeleteBuffers`, `IsBuffer` e
                    // `GetBufferParameteriv`, com os vetores e a lista de índices lendo de
                    // dentro do buffer. O nome é o `ARB`, que é o que os jogos procuram — o
                    // Prey Evil faz `strstr` de `ARB_vertex_buffer_object` na lista e desiste
                    // de instalar a função de desenho dele sem isso.
                    //
                    // O `point_size_array` continua **de fora**: dele só temos um `SUCCESS` que
                    // não faz nada, e anunciar o que não existe faz o jogo chamar função que
                    // não está lá.
                    gles::GL_EXTENSIONS => {
                        "GL_OES_draw_texture GL_ATI_imageon_misc GL_ATI_texture_compression_atitc \
                         GL_ARB_vertex_buffer_object "
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
                    self.gl.delete_texture(name);
                }
            }

            // --- Objetos de buffer ------------------------------------------------------
            // O `GL_ARB_vertex_buffer_object`, que no OpenGL ES 1.1 já é núcleo. Não é enfeite
            // de desempenho: o Prey Evil confere as duas extensões que quer na lista do
            // `glGetString`, e **só instala a função de desenho dele se achar as duas**. Sem o
            // `vertex_buffer_object` na lista, ele deixa o ponteiro em `[obj+0x274]` valendo
            // zero e em seguida o chama sem conferir — o salto para o endereço zero que
            // aparecia no relatório dele como "para no laço".
            "BindBuffer" => {
                let (alvo, nome) = (a[0], a[1]);
                // Ligar um nome que o `glGenBuffers` nunca entregou é legal no OpenGL: o nome
                // passa a existir, vazio, na primeira ligação.
                if nome != 0 {
                    self.gl_buffers.entry(nome).or_default();
                }
                match alvo {
                    gles::GL_ARRAY_BUFFER => self.gl_array_buffer = nome,
                    gles::GL_ELEMENT_ARRAY_BUFFER => self.gl_element_buffer = nome,
                    _ => {}
                }
            }
            "BufferData" | "BufferDataQUALCOMM" | "BufferDataATI" => {
                let (alvo, tamanho, dados) = (a[0], a[1], a[2]);
                let Some(nome) = self.buffer_ligado(alvo) else {
                    return Ok(Some(SUCCESS));
                };
                // `data` nulo é pedido legítimo: reserva o tamanho e deixa o conteúdo por
                // definir. O `glBufferSubData` vem depois preencher.
                // O tamanho é do jogo: `glBufferData(alvo, 0x7fffffff, NULL, uso)` pede 2 GiB
                // numa linha que cabe no guest, e o `vec!` correspondente aborta o processo. O
                // teto é o mesmo das leituras, e pelo mesmo motivo.
                let conteudo = if dados == 0 {
                    vec![0u8; tamanho_do_guest(tamanho as usize)?]
                } else {
                    self.read_bytes(dados, tamanho)?
                };
                self.gl_buffers.insert(nome, conteudo);
            }
            "BufferSubData" | "BufferSubDataQUALCOMM" => {
                let (alvo, inicio, tamanho, dados) = (a[0], a[1], a[2], a[3]);
                let Some(nome) = self.buffer_ligado(alvo) else {
                    return Ok(Some(SUCCESS));
                };
                let novo = self.read_bytes(dados, tamanho)?;
                if let Some(buffer) = self.gl_buffers.get_mut(&nome) {
                    let fim = inicio as usize + novo.len();
                    // Escrever fora do buffer é erro do jogo, e o OpenGL manda ignorar em vez
                    // de crescer: crescer esconderia o defeito e mudaria o `GL_BUFFER_SIZE`.
                    if fim <= buffer.len() {
                        buffer[inicio as usize..fim].copy_from_slice(&novo);
                    }
                }
            }
            "DeleteBuffers" | "DeleteBuffersQUALCOMM" => {
                for i in 0..a[0] {
                    let nome = self.cpu.read_u32(a[1] + i * 4)?;
                    self.gl_buffers.remove(&nome);
                    // Apagar um buffer desliga a ligação dele, e é o que a especificação pede:
                    // o alvo volta para zero, ou seja, para a memória do jogo.
                    if self.gl_array_buffer == nome {
                        self.gl_array_buffer = 0;
                    }
                    if self.gl_element_buffer == nome {
                        self.gl_element_buffer = 0;
                    }
                }
            }
            "IsBuffer" | "IsBufferQUALCOMM" => {
                let existe = u32::from(self.gl_buffers.contains_key(&a[0]));
                answer = Some((1, existe));
            }
            "GetBufferParameteriv" => {
                let (alvo, pname, saida) = (a[0], a[1], a[2]);
                let tamanho = self
                    .buffer_ligado(alvo)
                    .and_then(|nome| self.gl_buffers.get(&nome))
                    .map_or(0, |buffer| buffer.len() as u32);
                let valor = match pname {
                    gles::GL_BUFFER_SIZE => tamanho,
                    // O uso declarado não muda nada aqui, e devolver o que o jogo pediu seria
                    // inventar: zero é o que o OpenGL define para buffer sem uso declarado.
                    gles::GL_BUFFER_USAGE => 0,
                    _ => 0,
                };
                if saida != 0 {
                    self.cpu.write_u32(saida, valor)?;
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
                // A contagem de limpezas conta mesmo pulando — é diagnóstico do jogo, não do
                // quadro que a tela mostrou. O que pula é só o preenchimento de verdade.
                if !self.pula_desenho || self.gl_leitura_de_pixels {
                    self.gl.clear(a[0]);
                }
            }
            "ClearColorx" | "ClearColor" => {
                let c = std::array::from_fn(|i| number(a[i]));
                self.gl.set_clear_color(c);
            }
            // `ClearDepth` já recebe a profundidade em `[0, 1]`, que é a faixa do buffer.
            "ClearDepthx" | "ClearDepthf" => self.gl.set_clear_depth(number(a[0]).clamp(0.0, 1.0)),
            // O OpenGL ES 1.1 limita a cor corrente a [0, 1] quando ela é definida. O Alien Breaker
            // Deluxe pinta com `glColor4f(255, 255, 255, a)`: sem o limite, a textura era
            // multiplicada por 255 e o título e os menus estouravam para o branco.
            "Color4x" | "Color4f" => {
                let c = std::array::from_fn(|i| number(a[i]).clamp(0.0, 1.0));
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
            "DepthRangex" | "DepthRangef" => {
                let fixo = name.ends_with('x');
                self.gl.set_depth_range(escalar(a[0], fixo), escalar(a[1], fixo));
            }
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
            // O modo e os parâmetros do `GL_COMBINE` são enums, e chegam inteiros mesmo pela
            // variante de ponto fixo ou de `float`; só as escalas são número. Ver `TexEnv`.
            "TexEnvx" | "TexEnvi" | "TexEnvf" if a[0] == gles::GL_TEXTURE_ENV => {
                let numero = match name {
                    "TexEnvx" => gles::fixed(a[2]),
                    "TexEnvf" => f32::from_bits(a[2]),
                    _ => a[2] as i32 as f32,
                };
                let enumeracao = match name {
                    "TexEnvf" => f32::from_bits(a[2]) as u32,
                    _ => a[2],
                };
                match a[1] {
                    gles::GL_TEXTURE_ENV_MODE => self.gl.set_texture_env(enumeracao),
                    pname => self.gl.set_texture_env_param(pname, enumeracao, numero),
                }
            }
            "TexEnvxv" | "TexEnviv" | "TexEnvfv" if a[0] == gles::GL_TEXTURE_ENV => {
                match a[1] {
                    gles::GL_TEXTURE_ENV_COLOR => {
                        let mut cor = [0.0f32; 4];
                        for (i, canal) in cor.iter_mut().enumerate() {
                            let bruto = self.cpu.read_u32(a[2] + i as u32 * 4)?;
                            *canal = match name {
                                "TexEnvxv" => gles::fixed(bruto),
                                "TexEnvfv" => f32::from_bits(bruto),
                                // Inteiros vão de 0 ao maior positivo, como toda cor inteira.
                                _ => (bruto as i32 as f32 / i32::MAX as f32).max(0.0),
                            };
                        }
                        self.gl.set_texture_env_color(cor);
                    }
                    gles::GL_TEXTURE_ENV_MODE => {
                        let value = self.cpu.read_u32(a[2])?;
                        self.gl.set_texture_env(value);
                    }
                    pname => {
                        let bruto = self.cpu.read_u32(a[2])?;
                        let (enumeracao, numero) = match name {
                            "TexEnvxv" => (bruto, gles::fixed(bruto)),
                            "TexEnvfv" => (f32::from_bits(bruto) as u32, f32::from_bits(bruto)),
                            _ => (bruto, bruto as i32 as f32),
                        };
                        self.gl.set_texture_env_param(pname, enumeracao, numero);
                    }
                }
            }
            // O ambiente de ponto (`GL_POINT_SPRITE_OES`) não tem nada que desenhemos.
            "TexEnvx" | "TexEnvi" | "TexEnvf" | "TexEnvxv" | "TexEnviv" | "TexEnvfv" => {}
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
            // A névoa. O `GL_FOG_MODE` chega como número — `GL_LINEAR`, `GL_EXP` ou `GL_EXP2`
            // —, e nas formas `x` ele vem **inteiro**, não em ponto fixo: é uma enumeração, e
            // convertê-la como escala daria `0x2601/65536`, que não é modo nenhum.
            "Fogxv" | "Fogfv" => {
                let valores = match a[0] == gles::GL_FOG_MODE {
                    true => [self.cpu.read_u32(a[1])? as f32, 0.0, 0.0, 0.0],
                    false => self.le_parametro(a[0], a[1], name.ends_with("xv"))?,
                };
                self.gl.set_fog(a[0], valores);
            }
            "Fogx" | "Fogf" => {
                let valor = match a[0] == gles::GL_FOG_MODE {
                    true => a[1] as f32,
                    false => escalar(a[1], name.ends_with('x')),
                };
                self.gl.set_fog(a[0], [valor, 0.0, 0.0, 0.0]);
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
                    buffer: self.gl_array_buffer,
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
                    buffer: self.gl_array_buffer,
                };
                // O vetor de coordenadas pertence à unidade escolhida pelo
                // `glClientActiveTexture`; só as duas primeiras desenham.
                let slot = match (name, self.gl.client_unit()) {
                    ("VertexPointer", _) => &mut self.gl_vertices,
                    ("ColorPointer", _) => &mut self.gl_colors,
                    (_, 0) => &mut self.gl_texcoords,
                    (_, 1) => &mut self.gl_texcoords1,
                    _ => return Ok(Some(SUCCESS)),
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
                    gles::GL_TEXTURE_COORD_ARRAY => match self.gl.client_unit() {
                        0 => self.gl_texcoords.enabled = on,
                        1 => self.gl_texcoords1.enabled = on,
                        _ => {}
                    },
                    _ => {}
                }
            }
            "DrawArrays" => {
                let indices: Vec<u32> = (0..a[2]).map(|i| a[1] + i).collect();
                self.gles_draw(a[0], &indices)?;
            }
            "DrawElements" => {
                let (mode, count, kind, list) = (a[0], a[1], a[2], a[3]);
                // A lista inteira num pedido só, pelo mesmo motivo do `read_array`: cada
                // travessia para o backend custa mais que os dois bytes que ela traz.
                let largura = if kind == gles::GL_UNSIGNED_BYTE { 1 } else { 2 };
                // Com um buffer de índices ligado, `list` é deslocamento dentro dele — e aqui
                // vale a ligação **corrente**, ao contrário dos vetores de vértice.
                let bytes =
                    self.bytes_do_vetor(self.gl_element_buffer, list, count * largura)?;
                let indices: Vec<u32> = bytes
                    .chunks_exact(largura as usize)
                    .map(|c| {
                        if largura == 1 {
                            c[0] as u32
                        } else {
                            u16::from_le_bytes([c[0], c[1]]) as u32
                        }
                    })
                    .collect();
                self.gles_draw(mode, &indices)?;
            }

            "GetIntegerv" | "GetFixedv" | "GetBooleanv" => {
                let (largura, altura) = self.gl.frame_size();
                let (width, height) = (largura as i32, altura as i32);
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
            // glScissor(x, y, width, height) — o retângulo fora do qual nada é desenhado, com o
            // `y` de baixo para cima, como a viewport.
            //
            // O Peggle desenha a folha de fontes inteira e aperta a tesoura para aparecer uma
            // letra só. Ignorada, a folha inteira ia para a tela por cima do jogo.
            "Scissor" => self
                .gl
                .set_scissor(a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32),
            // glPixelStorei(pname, param) — por enquanto só o alinhamento de linha na subida de
            // textura. **É a única chamada de GL que o levantamento das 62 ROMs flagrou como
            // "atendida sem fazer nada"**: o Double Dragon, o Galaxy on Fire, o Pac-Mania, o
            // Powerboat Challenge e o Raging Thunder 2 a usam, e ignorá-la desloca as linhas de
            // uma textura cuja largura em bytes não é múltipla de quatro — a imagem sai
            // embaralhada em diagonal, e sem uma linha de aviso em lugar nenhum.
            "PixelStorei" => {
                if a[0] == gles::GL_UNPACK_ALIGNMENT {
                    self.unpack_alignment = match a[1] {
                        // Só estes quatro valores são legais no GL; qualquer outro é erro do jogo,
                        // e ficar com o que estava é mais seguro que passar a ler torto.
                        1 | 2 | 4 | 8 => a[1],
                        _ => self.unpack_alignment,
                    };
                }
            }
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
        // Os dezesseis de uma vez: eram dezesseis travessias para o backend a cada
        // `LoadMatrix`/`MultMatrix`, e a matriz é contígua por definição. Se qualquer parte
        // dela estiver fora do mapa, a leitura falha — como falhava antes, no primeiro
        // componente ruim.
        let mut bytes = [0u8; 64];
        self.cpu.read_mem(address, &mut bytes)?;
        let mut m = rasterizer::IDENTITY;
        for (slot, palavra) in m.iter_mut().zip(bytes.chunks_exact(4)) {
            let word = u32::from_le_bytes([palavra[0], palavra[1], palavra[2], palavra[3]]);
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
                self.anota_ponto_ruim(format!(
                    "textura paletizada {format:#x} sem paleta completa"
                ));
                return Ok(());
            };
            let name = self.gl.bound_texture();
            // A paletizada traz a cadeia inteira num bloco só, e o `level` dela conta os
            // mipmaps em vez de nomeá-los; o decodificador devolve o nível base.
            self.gl
                .upload_level(name, 0, width as usize, height as usize, decoded);
            return Ok(());
        }
        let explicit_alpha = match format {
            gles::GL_ATC_RGB_AMD => false,
            gles::GL_ATC_RGBA_EXPLICIT_ALPHA_AMD => true,
            _ => {
                self.anota_ponto_ruim(format!("textura comprimida no formato {format:#x}"));
                return Ok(());
            }
        };
        if width == 0 || height == 0 || pixels == 0 {
            return Ok(());
        }
        let bytes = self.read_bytes(pixels, size)?;
        let decoded = atc::decode(&bytes, width as usize, height as usize, explicit_alpha);

        let name = self.gl.bound_texture();
        self.gl
            .upload_level(name, level, width as usize, height as usize, decoded);
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
            let bytes = self.le_texels(pixels, width, height, format, kind)?;
            decode_texels(&bytes, format, kind, texels)
        };
        // Os parâmetros de repetição e filtro sobrevivem a uma nova imagem: no OpenGL eles são
        // do nome da textura, não do conteúdo, e o jogo costuma defini-los uma vez só.
        let name = self.gl.bound_texture();
        self.gl
            .upload_level(name, level, width as usize, height as usize, decoded);
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
        // A partir daqui frameskip não pode mais pular draw/clear: este jogo observa o
        // framebuffer, e entregar a imagem anterior deixa de ser perda visual e vira dado errado
        // na memória do guest.
        self.gl_leitura_de_pixels = true;
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
                self.anota_ponto_ruim(format!(
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
    /// Lê os texels de uma imagem do guest, respeitando o alinhamento de linha.
    ///
    /// `glPixelStorei(GL_UNPACK_ALIGNMENT, n)` diz com quantos bytes cada linha começa alinhada na
    /// memória do jogo — o padrão do OpenGL é 4. Sem isto, uma textura de largura ímpar em bytes
    /// chega com as linhas deslocadas duas a duas: a imagem sai embaralhada em diagonal, sem aviso.
    fn le_texels(
        &mut self,
        pixels: u32,
        width: u32,
        height: u32,
        format: u32,
        kind: u32,
    ) -> Result<Vec<u8>, CpuError> {
        let por_texel = bytes_per_texel(format, kind);
        let apertado = width * por_texel;
        let passo = arredonda_para(apertado, self.unpack_alignment);
        if passo == apertado {
            return self.read_bytes(pixels, apertado * height);
        }
        // **Uma leitura só**, do bloco alinhado inteiro, e as linhas são compactadas depois: pedir
        // linha a linha são `height` travessias até a memória do guest, e cada travessia custa
        // mais que os bytes que traz.
        let bloco = self.read_bytes(pixels, passo * height)?;
        let mut saida = Vec::with_capacity((apertado * height) as usize);
        for linha in 0..height as usize {
            let inicio = linha * passo as usize;
            saida.extend_from_slice(&bloco[inicio..inicio + apertado as usize]);
        }
        Ok(saida)
    }

    pub(super) fn gles_tex_sub_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, x, y, width, height) = (a[1], a[2], a[3], a[4], a[5]);
        let (format, kind, pixels) = (a[6], a[7], a[8]);
        if level != 0 || width == 0 || height == 0 || pixels == 0 {
            return Ok(());
        }
        let texels = (width * height) as usize;
        let bytes = self.le_texels(pixels, width, height, format, kind)?;
        let novos = decode_texels(&bytes, format, kind, texels);

        let name = self.gl.bound_texture();
        if let Err(Some((tw, th))) = self.gl.sub_image(name, x, y, width, height, &novos) {
            self.anota_ponto_ruim(format!(
                "TexSubImage2D de {width}x{height} em ({x},{y}) não cabe numa textura {tw}x{th}"
            ));
        }
        Ok(())
    }

    /// Monta os vértices a partir dos vetores do cliente e manda desenhar.
    pub(super) fn gles_draw(&mut self, mode: u32, indices: &[u32]) -> Result<(), CpuError> {
        if !self.gl_vertices.em_uso() || indices.is_empty() {
            return Ok(());
        }
        // **O quadro pulado sai daqui, antes de qualquer leitura de memória do guest.** É o que
        // faz o pulo economizar de verdade: sem isto, o custo caro — atravessar a FFI do unicorn
        // para trazer cada vértice — aconteceria do mesmo jeito, e só a rasterização sumiria. O
        // jogo não vê diferença nenhuma: hardware real também não avisa se o pixel chegou à tela.
        if self.pula_desenho && !self.gl_leitura_de_pixels {
            return Ok(());
        }
        let base = self.gl.current_color();
        // Um bloco por array, não um por componente: é a mesma memória do guest, pedida de
        // uma vez. Ver [`Self::read_array`].
        let posicoes = self.read_array(self.gl_vertices, indices, [0.0, 0.0, 0.0, 1.0])?;
        let cores = self
            .gl_colors
            .em_uso()
            .then(|| self.read_array(self.gl_colors, indices, [0.0, 0.0, 0.0, 1.0]))
            .transpose()?;
        let uvs = self
            .gl_texcoords
            .em_uso()
            .then(|| self.read_array(self.gl_texcoords, indices, [0.0; 4]))
            .transpose()?;
        let uvs1 = self
            .gl_texcoords1
            .em_uso()
            .then(|| self.read_array(self.gl_texcoords1, indices, [0.0; 4]))
            .transpose()?;
        let normais = self
            .gl_normals
            .em_uso()
            .then(|| self.read_array(self.gl_normals, indices, [0.0, 0.0, 1.0, 0.0]))
            .transpose()?;
        let vertices: Vec<Vertex> = (0..indices.len())
            .map(|i| Vertex {
                position: posicoes[i],
                color: cores.as_ref().map_or(base, |c| c[i]),
                uv: uvs.as_ref().map_or([0.0; 2], |t| [t[i][0], t[i][1]]),
                uv1: uvs1.as_ref().map_or([0.0; 2], |t| [t[i][0], t[i][1]]),
                normal: normais
                    .as_ref()
                    .map_or(self.gl_normal_atual, |n| [n[i][0], n[i][1], n[i][2]]),
                // Quem calcula o fator da névoa é a etapa de vértice, que é onde a distância em
                // coordenadas de olho existe.
                fog: 1.0,
            })
            .collect();
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
        let quantos = gles::componentes(pname).min(4);
        if quantos == 0 {
            return Ok(valores);
        }
        // Um pedido só para os até quatro componentes, pelo mesmo motivo do `read_array`.
        let mut bytes = [0u8; 16];
        self.cpu.read_mem(ponteiro, &mut bytes[..quantos * 4])?;
        for (valor, palavra) in valores.iter_mut().zip(bytes.chunks_exact(4)).take(quantos) {
            *valor = escalar(
                u32::from_le_bytes([palavra[0], palavra[1], palavra[2], palavra[3]]),
                fixo,
            );
        }
        Ok(valores)
    }

    /// Lê um elemento de um vetor do cliente, completando os componentes que faltam.
    /// Lê de uma vez o trecho do array que os índices cobrem, e decodifica dali.
    ///
    /// O caminho por componente atravessa o backend para copiar quatro bytes, e ele procura a
    /// região antes de copiar: **57 ns**, contra **0,3 ns** quando os mesmos
    /// quatro bytes vêm de um `read_mem` de um quilobyte. No Quake são 6,6 milhões de vértices
    /// em 15 segundos virtuais, cada um com posição e coordenada de textura — e isso era
    /// **3,2 s dos 3,6 s** que as draw calls custavam, contra 400 ms do rasterizador de fato.
    ///
    /// Se a leitura em bloco falhar, cai no caminho antigo. O trecho vai do primeiro ao último
    /// índice, e um array que termine colado no fim da região mapeada pode ter o espaço de
    /// `stride` do último elemento fora dela — o que valia antes continua valendo.
    /// Que nome está ligado num alvo de buffer. `None` quando é zero — que não é buffer
    /// nenhum, e sim "os ponteiros são endereços da memória do jogo".
    pub(super) fn buffer_ligado(&self, alvo: u32) -> Option<u32> {
        let nome = match alvo {
            gles::GL_ARRAY_BUFFER => self.gl_array_buffer,
            gles::GL_ELEMENT_ARRAY_BUFFER => self.gl_element_buffer,
            _ => 0,
        };
        (nome != 0).then_some(nome)
    }

    /// Bytes de um vetor, venham do objeto de buffer ou da memória do jogo.
    ///
    /// É o único lugar que sabe a diferença, e é de propósito: com um buffer ligado, o
    /// "ponteiro" que o jogo passou não é endereço nenhum — é deslocamento dentro do buffer.
    /// Ler a memória do jogo naquele número daria lixo ou falha de acesso, e foi o que o
    /// `glVertexPointer(…, 0)` de um jogo com buffer ligado faria: ler o endereço zero.
    fn bytes_do_vetor(
        &self,
        buffer: u32,
        endereco: u32,
        quantos: u32,
    ) -> Result<Vec<u8>, CpuError> {
        if buffer == 0 {
            return self.read_bytes(endereco, quantos);
        }
        let vazio = Vec::new();
        let conteudo = self.gl_buffers.get(&buffer).unwrap_or(&vazio);
        Ok(fatia_do_buffer(conteudo, endereco, quantos))
    }

    pub(super) fn read_array(
        &self,
        pointer: ArrayPointer,
        indices: &[u32],
        default: [f32; 4],
    ) -> Result<Vec<[f32; 4]>, CpuError> {
        let avulso = |maquina: &Self| -> Result<Vec<[f32; 4]>, CpuError> {
            indices
                .iter()
                .map(|&i| maquina.read_attribute(pointer, i, default))
                .collect()
        };
        let component = component_size(pointer.kind);
        let largura = pointer.size.min(4) * component;
        let stride = if pointer.stride == 0 {
            pointer.size * component
        } else {
            pointer.stride
        };
        let (Some(&menor), Some(&maior)) = (indices.iter().min(), indices.iter().max()) else {
            return Ok(Vec::new());
        };
        // Em u64 porque `maior * stride` estoura u32 num ponteiro corrompido, e aí o teto
        // abaixo é justamente quem precisa enxergar o número grande para recusar.
        let extensao = (maior - menor) as u64 * stride as u64 + largura as u64;
        let inicio = pointer.address as u64 + menor as u64 * stride as u64;
        if extensao > TETO_DO_ARRAY || inicio + extensao > u32::MAX as u64 {
            return avulso(self);
        }
        let Ok(bytes) = self.bytes_do_vetor(pointer.buffer, inicio as u32, extensao as u32)
        else {
            return avulso(self);
        };
        Ok(indices
            .iter()
            .map(|&index| {
                let base = (index - menor) as usize * stride as usize;
                let mut out = default;
                for i in 0..pointer.size.min(4) as usize {
                    let em = base + i * component as usize;
                    out[i] = match pointer.kind {
                        gles::GL_FLOAT => f32::from_le_bytes([
                            bytes[em],
                            bytes[em + 1],
                            bytes[em + 2],
                            bytes[em + 3],
                        ]),
                        gles::GL_FIXED => gles::fixed(u32::from_le_bytes([
                            bytes[em],
                            bytes[em + 1],
                            bytes[em + 2],
                            bytes[em + 3],
                        ])),
                        gles::GL_SHORT => i16::from_le_bytes([bytes[em], bytes[em + 1]]) as f32,
                        gles::GL_UNSIGNED_SHORT => {
                            u16::from_le_bytes([bytes[em], bytes[em + 1]]) as f32
                        }
                        gles::GL_BYTE => bytes[em] as i8 as f32,
                        // `GL_UNSIGNED_BYTE` só aparece em cor, e ali o valor é normalizado.
                        _ => bytes[em] as f32 / 255.0,
                    };
                }
                out
            })
            .collect())
    }

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
        let quantos = pointer.size.min(4);
        // Um pedido só para o elemento inteiro, em vez de um por componente: é o mesmo motivo
        // do `read_array`, e agora também o que deixa o buffer entrar por um caminho só.
        let bytes = self.bytes_do_vetor(
            pointer.buffer,
            pointer.address + index * stride,
            quantos * component,
        )?;
        let mut out = default;
        for i in 0..quantos as usize {
            let em = i * component as usize;
            let Some(campo) = bytes.get(em..em + component as usize) else {
                break;
            };
            out[i] = match pointer.kind {
                gles::GL_FLOAT => f32::from_le_bytes([campo[0], campo[1], campo[2], campo[3]]),
                gles::GL_FIXED => gles::fixed(u32::from_le_bytes([
                    campo[0], campo[1], campo[2], campo[3],
                ])),
                gles::GL_SHORT => i16::from_le_bytes([campo[0], campo[1]]) as f32,
                gles::GL_UNSIGNED_SHORT => u16::from_le_bytes([campo[0], campo[1]]) as f32,
                gles::GL_BYTE => campo[0] as i8 as f32,
                // `GL_UNSIGNED_BYTE` só aparece em cor, e ali o valor é normalizado.
                _ => campo[0] as f32 / 255.0,
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

    /// Fecha o quadro no `eglSwapBuffers`.
    ///
    /// **Pinta a fila e não lê o resultado.** São duas coisas diferentes, e separá-las é o ganho:
    ///
    /// - pintar a fila é a **rasterização** do quadro, e ela não tem como ser adiada — é o trabalho
    ///   que o jogo pediu;
    /// - ler o quadro de volta para a memória da CPU existe para o **desenho 2D por cima** e para
    ///   quando o guest pede os pixels. Um jogo de 3D puro não faz nenhuma das duas coisas, e no
    ///   portátil essa leitura obriga a GPU de tiles a terminar e devolver o quadro a cada troca.
    ///
    /// Então o quadro fica **pendente**, e [`Machine::materializa_quadro_gl`] o traz quando alguém
    /// precisar de verdade. Quem decide é o caminho de desenho: qualquer chamada de 2D materializa
    /// antes de escrever, porque escrever por cima de um quadro velho apagaria a cena.
    pub(super) fn present_gl(&mut self) {
        self.gl.descarrega_o_desenho();
        self.gl_quadro_pendente = true;
        // **Só a placa adia.** No rasterizador de processador a leitura é uma conversão em
        // memória: não há espera a economizar, e o frontend lê a tela todo quadro de qualquer
        // jeito — adiar ali só criaria a chance de ele apresentar um quadro velho.
        if !self.gl.quadro_espera_pela_placa() {
            self.materializa_quadro_gl();
        }
    }

    /// Chamadas de estado enviadas à placa e quantas o espelho poupou. Ver
    /// [`crate::video::gpu::Espelho`].
    pub fn estado_enviado_e_poupado(&self) -> (u64, u64) {
        self.gl.estado_enviado_e_poupado()
    }

    /// Quantas vezes o quadro da placa foi trazido para a tela da CPU nesta sessão.
    ///
    /// Comparado com [`Machine::gl_swaps`] diz o quanto o adiamento rendeu: cada troca de buffer
    /// sem materialização é uma leitura de quadro que **não** aconteceu. Serve de número de
    /// conferência no portátil, onde a leitura é a cara: ver [`Machine::present_gl`].
    pub fn materializacoes_do_quadro_gl(&self) -> u32 {
        self.gl_materializacoes
    }

    /// Traz para a tela da CPU o quadro que o `eglSwapBuffers` deixou pendente.
    ///
    /// Chamado por todo caminho que **lê ou escreve** a tela do console: as três interfaces de
    /// desenho 2D, a leitura de pixels e o despejo de diagnóstico. Não é chamado pela janela nem
    /// pelo core quando eles apresentam a textura da placa — esses não querem os pixels, querem a
    /// textura, e o readback existia para eles por engano.
    pub fn materializa_quadro_gl(&mut self) {
        if !self.gl_quadro_pendente {
            return;
        }
        self.gl_quadro_pendente = false;
        self.gl_materializacoes = self.gl_materializacoes.saturating_add(1);
        // A tela pode ser a superfície do "device bitmap", quando o jogo pediu uma — é ela que
        // vale, e não o framebuffer de reserva.
        let (width, height) = {
            let target = self.screen();
            (target.width() as usize, target.height() as usize)
        };
        // O quadro vai direto para o buffer do anterior, em RGB565: sem o vetor de `u16` e a volta
        // para bytes, e sem conversão nenhuma quando nada foi desenhado desde o último.
        let mut words = std::mem::take(&mut self.gl_last_frame_words);
        self.gl.frame_rgb565_words(width, height, &mut words);
        match self.bitmaps.get_mut(&self.device_bitmap) {
            Some(surface) => surface.load_rgb565_words(&words),
            None => self.screen.load_rgb565_words(&words),
        }
        self.gl_last_frame_words = words;
        // **A marca é do instante em que a tela ficou igual ao quadro 3D**, e é por isso que ela
        // vem aqui e não na troca de buffer: `quadro_na_placa` compara esta contagem com a de
        // agora para saber se algum 2D desenhou por cima depois disso.
        self.escritas_do_quadro_gl = Some(self.screen().escritas());
    }

    /// O quadro 3D na resolução interna, quando é ele que está na tela.
    ///
    /// Só vale enquanto a tela é exatamente o que o último `eglSwapBuffers` pôs lá: qualquer
    /// desenho 2D depois disso — um HUD pelo `IDisplay`, uma caixa de mensagem — vive só na tela
    /// do console, e mostrar a textura grande o apagaria. Nesse caso a janela fica com a tela de
    /// 640×480, como sempre.
    pub fn quadro_na_placa(&self) -> Option<crate::video::rasterizer::QuadroNaPlaca> {
        let intacta = self.escritas_do_quadro_gl == Some(self.screen().escritas());
        intacta.then(|| self.gl.quadro_na_placa()).flatten()
    }

    /// O quadro 3D na resolução interna, como superfície, para gravar sem janela.
    pub fn quadro_grande(&mut self) -> Option<Framebuffer> {
        let (w, h, rgba) = self.gl.le_quadro_grande()?;
        let mut quadro = Framebuffer::new(w as u32, h as u32);
        for (i, p) in rgba.chunks_exact(4).enumerate() {
            let cor = Rgb { r: p[0], g: p[1], b: p[2] };
            quadro.set_pixel((i % w) as i32, (i / w) as i32, cor);
        }
        Some(quadro)
    }

    /// A resolução interna do rasterizador da placa. Ver [`Rasterizador::define_escala`].
    pub fn define_resolucao_interna(&mut self, escala: usize) {
        self.gl.define_escala(escala);
    }

    /// Faz o desenho sair no framebuffer do frontend, quando ele entrega um.
    ///
    /// É o que o `libretro` pede de quem usa render em hardware: o core desenha no framebuffer
    /// que o frontend indica a cada quadro, e é ele que apresenta. Sem framebuffer de fora, o
    /// motor desenha no próprio e o quadro sai pelo `frame_rgb565`, como sempre.
    pub fn desenha_no_fbo(&mut self, fbo: Option<u32>) {
        self.gl.desenha_no_fbo(fbo);
    }

    /// Diz ao rasterizador de placa para descartar profundidade e estêncil depois do quadro.
    /// Ver [`Rasterizador::define_descarte_de_tiles`].
    pub fn define_descarte_de_tiles(&mut self, descartar: bool) {
        self.gl.define_descarte_de_tiles(descartar);
    }

    /// Reduz a resolução interna do 3D no rasterizador de processador. Ver
    /// [`Rasterizador::define_reducao`].
    pub fn define_reducao(&mut self, reducao: usize) {
        self.gl.define_reducao(reducao);
    }

    /// A proporção experimental do 3D. Ver [`Rasterizador::define_proporcao`].
    pub fn define_proporcao(&mut self, aspecto: Option<f32>) {
        self.gl.define_proporcao(aspecto);
    }

    /// As melhorias de imagem do rasterizador da placa: antialias e filtro anisotrópico. Ver
    /// [`Rasterizador::define_antialias`] e [`Rasterizador::define_anisotropico`].
    pub fn define_melhorias(&mut self, amostras: usize, anisotropico: usize) {
        self.gl.define_antialias(amostras);
        self.gl.define_anisotropico(anisotropico);
    }

    /// Se a névoa do jogo vale. Escolha de quem joga, não do jogo — ver
    /// [`rasterizer::Rasterizador::define_neblina`].
    pub fn define_neblina(&mut self, permitida: bool) {
        self.gl.define_neblina(permitida);
    }

    /// Se o quadro de agora deve pular o desenho — ver o campo `pula_desenho`.
    ///
    /// Chamado uma vez por quadro, antes do jogo rodar: a decisão vale para todo `gles_draw` e
    /// `Clear` que acontecerem enquanto o CPU emula este quadro, e é reavaliada no próximo.
    pub fn define_pula_desenho(&mut self, pula: bool) {
        self.pula_desenho = pula;
    }

    /// Se o jogo leu pixels do framebuffer e portanto desabilitou frameskip de rasterização.
    pub fn leu_pixels(&self) -> bool {
        self.gl_leitura_de_pixels
    }

    /// Devolve ao dono o estado de GL que o rasterizador mexeu. Ver
    /// [`rasterizer::Rasterizador::devolve_o_contexto`].
    pub fn devolve_o_contexto(&self) {
        self.gl.devolve_o_contexto();
    }
}

/// A fatia de um objeto de buffer que um vetor pede, ou zeros quando ela não cabe.
///
/// Separada da máquina por ser a única regra do caminho de buffer que dá para errar sozinha —
/// e por dar para cobrar num teste sem levantar um núcleo ARM inteiro.
///
/// Fora do buffer **não** é motivo para derrubar o desenho: o OpenGL deixa o resultado
/// indefinido, e zero é o indefinido mais inofensivo que dá para escolher. Devolver erro aqui
/// abortaria o `glDrawElements` inteiro por causa de um vértice, e um jogo que suba a malha em
/// pedaços passa por esse caso sem estar com defeito.
fn fatia_do_buffer(conteudo: &[u8], endereco: u32, quantos: u32) -> Vec<u8> {
    let inicio = endereco as usize;
    let fim = inicio.saturating_add(quantos as usize);
    match conteudo.get(inicio..fim) {
        Some(fatia) => fatia.to_vec(),
        None => vec![0u8; quantos as usize],
    }
}

#[cfg(test)]
mod testes_do_buffer {
    use super::fatia_do_buffer;

    #[test]
    fn o_deslocamento_zero_e_o_comeco_do_buffer() {
        // É o caso que mais aparece: com um buffer ligado, `glVertexPointer(…, 0)` não é
        // ponteiro nulo — é "do começo do buffer". Ler a memória do jogo no endereço zero era
        // exatamente o que fazia um jogo com VBO morrer em acesso inválido.
        let conteudo = [1u8, 2, 3, 4, 5, 6];
        assert_eq!(fatia_do_buffer(&conteudo, 0, 3), vec![1, 2, 3]);
    }

    #[test]
    fn o_deslocamento_anda_dentro_do_buffer() {
        let conteudo = [1u8, 2, 3, 4, 5, 6];
        assert_eq!(fatia_do_buffer(&conteudo, 4, 2), vec![5, 6]);
    }

    #[test]
    fn o_que_passa_do_fim_sai_zerado_em_vez_de_falhar() {
        let conteudo = [1u8, 2, 3, 4];
        assert_eq!(fatia_do_buffer(&conteudo, 3, 4), vec![0, 0, 0, 0]);
        assert_eq!(fatia_do_buffer(&conteudo, 9, 2), vec![0, 0]);
        // E um buffer que nem foi preenchido responde igual, sem caso especial.
        assert_eq!(fatia_do_buffer(&[], 0, 3), vec![0, 0, 0]);
    }

    #[test]
    fn o_deslocamento_grande_nao_estoura_a_soma() {
        // `endereco + quantos` em `usize` de 32 bits daria a volta e a fatia passaria pelo
        // teste de limite. O `saturating_add` é o que impede isso — e o caso existe: um
        // ponteiro de vetor corrompido chega aqui como deslocamento enorme.
        assert_eq!(fatia_do_buffer(&[1, 2, 3], u32::MAX, 8), vec![0u8; 8]);
    }
}
