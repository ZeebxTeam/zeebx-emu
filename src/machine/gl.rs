//! OpenGL ES 1.1: call dispatch and the bridge to the rasterizer.

use super::*;

/// Cap on the span the block read agrees to assemble at once. A corrupted array pointer gives an
/// absurd extent, and allocating it would trade slowness for running out of memory. Above this,
/// it goes back to reading component by component — which is slow, but safe.
const ARRAY_CAP: u64 = 8 << 20;

/// The names in use for one kind of OpenGL ES object: textures or buffers.
///
/// **A deleted name becomes valid again.** Names used to come from a counter that only grew, and
/// the specification does not forbid that: it only requires `glGen*` to hand out names not in use.
/// But Rally Master Pro keeps a flag per texture in a 248-byte array indexed by the texture's
/// name, and each stage deletes and recreates the scene's textures. With the counter, the name
/// went past 248 after a few races and the game wrote beyond the array, over the neighboring
/// object on the heap. That neighbor's destructor later read a pointer made of `0x01` bytes and
/// the game stopped with "invalid access at `0x0101010d`". Measured: with a guard after each
/// block, the stray writes all landed at the end of that array.
///
/// The **smallest** free name comes out. Which name the Zeebo driver hands out was not measured on
/// the console, only that it reuses them: the game runs on the console without crashing. Smallest
/// free is what keeps names low, and low names are what Rally's array assumes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct GlNames {
    in_use: std::collections::BTreeSet<u32>,
}

impl GlNames {
    /// The names from a list, as a save state stores them.
    pub(crate) fn from(names: impl IntoIterator<Item = u32>) -> Self {
        Self {
            in_use: names.into_iter().filter(|&name| name != 0).collect(),
        }
    }

    /// The smallest free name, which becomes in use. Zero is the default object and never comes
    /// out of here.
    pub(crate) fn generate(&mut self) -> u32 {
        let mut name = 1;
        for &used in &self.in_use {
            if used != name {
                break;
            }
            name += 1;
        }
        self.in_use.insert(name);
        name
    }

    /// A name the game bound without generating it: OpenGL creates it on the first bind, and from
    /// then on `glGen*` cannot hand it to anyone else.
    pub(crate) fn mark_used(&mut self, name: u32) {
        if name != 0 {
            self.in_use.insert(name);
        }
    }

    pub(crate) fn free(&mut self, name: u32) {
        self.in_use.remove(&name);
    }

    pub(crate) fn in_use(&self) -> impl Iterator<Item = u32> + '_ {
        self.in_use.iter().copied()
    }

    /// The largest name in use, or zero.
    pub(crate) fn largest(&self) -> u32 {
        self.in_use.last().copied().unwrap_or(0)
    }
}

impl<C: CpuBackend> Machine<C> {
    /// A texture setting coming from the extension, with the OpenGL ES names.
    pub(super) fn apply_texture_setting(&mut self, name: &str, pname: u32, value: u32) {
        match name.starts_with("TexEnv") {
            true if pname == gles::GL_TEXTURE_ENV_MODE => self.gl.set_texture_env(value),
            true => {}
            false => self.gl.set_texture_parameter(pname, value),
        }
    }

    /// The GL calls we serve without doing anything.
    pub fn ignored_gl(&self) -> Vec<&'static str> {
        self.ignored_gl.iter().copied().collect()
    }

    /// OpenGL ES, in the two forms BREW exposes it in.
    ///
    /// The new one is `IGLES11` from `sdk/inc/AEEGLES10.h` and `AEEGLES11.h`; the old one is `IGL`
    /// from `sdk/inc/AEEGL.h`, which does not take `this` and returns the result directly. As with
    /// EGL, once the `gl` prefix is stripped the names match, and a single translator serves both.
    ///
    /// Nothing is drawn here: the arguments become state or vertices, and the one that rasterizes
    /// is the [`rasterizer`].
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
        // The `x` variants take 16.16 fixed point and the `f` ones a 32-bit `float` — same
        // function, only how the number arrives changes.
        let fixed = |v: u32| gles::fixed(v);
        let float = |v: u32| f32::from_bits(v);
        let number = |v: u32| {
            if name.ends_with('x') {
                fixed(v)
            } else {
                float(v)
            }
        };
        // The few methods that produce a value: in the old form it is the return value, in the
        // new one it goes to an output pointer.
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
                    // Only what really exists. Advertising an extension we don't have would
                    // make the game call a function that doesn't exist — and leaving out one we
                    // do have is even worse: the console's ten arcade ports check for
                    // `GL_OES_draw_texture` here and give up on graphics initialization without
                    // it.
                    // `atitc` went in because we really decode it — see `atc.rs` and
                    // `gles_compressed_tex_image`. The Z-Wheel looks for it before building the
                    // stage, whose textures (`stage_*.qxt`) are ATITC; without the name in the
                    // list it gives up on the whole stage.
                    //
                    // `vertex_buffer_object` went in when it came to exist: `BindBuffer`,
                    // `BufferData`, `BufferSubData`, `DeleteBuffers`, `IsBuffer` and
                    // `GetBufferParameteriv`, with the arrays and the index list reading from
                    // inside the buffer. The name is the `ARB` one, which is what games look
                    // for — Prey Evil does a `strstr` for `ARB_vertex_buffer_object` in the list
                    // and gives up on installing its draw function without it.
                    //
                    // `point_size_array` stays **out**: all we have of it is a `SUCCESS` that
                    // does nothing, and advertising what doesn't exist makes the game call a
                    // function that isn't there.
                    gles::GL_EXTENSIONS => {
                        "GL_OES_draw_texture GL_OES_query_matrix GL_ATI_imageon_misc \
                         GL_ATI_texture_compression_atitc GL_ARB_vertex_buffer_object "
                    }
                    _ => "",
                };
                let addr = self.intern(text)?;
                answer = Some((1, addr));
            }
            // With nowhere to write, no name comes out: the game would have no way to give it
            // back. See [`GlNames`].
            "GenTextures" | "GenBuffers" if a[1] != 0 => {
                let (count, out) = (a[0], a[1]);
                for i in 0..count {
                    let gl_name = match name == "GenTextures" {
                        true => self.texture_names.generate(),
                        false => self.buffer_names.generate(),
                    };
                    self.cpu.write_u32(out + i * 4, gl_name)?;
                }
            }
            "GenTextures" | "GenBuffers" => {}
            "DeleteTextures" => {
                for i in 0..a[0] {
                    let name = self.cpu.read_u32(a[1] + i * 4)?;
                    self.gl.delete_texture(name);
                    self.texture_names.free(name);
                }
            }

            // --- Buffer objects ---------------------------------------------------------
            // `GL_ARB_vertex_buffer_object`, which in OpenGL ES 1.1 is already core. It is not a
            // performance nicety: Prey Evil checks for the two extensions it wants in the
            // `glGetString` list, and **only installs its draw function if it finds both**.
            // Without `vertex_buffer_object` in the list, it leaves the pointer at `[obj+0x274]`
            // as zero and then calls it without checking — the jump to address zero that showed
            // up in its report as "stops in the loop".
            "BindBuffer" => {
                let (target, gl_name) = (a[0], a[1]);
                // Binding a name that `glGenBuffers` never handed out is legal in OpenGL: the
                // name comes into existence, empty, on the first bind.
                if gl_name != 0 {
                    self.gl_buffers.entry(gl_name).or_default();
                }
                self.buffer_names.mark_used(gl_name);
                match target {
                    gles::GL_ARRAY_BUFFER => self.gl_array_buffer = gl_name,
                    gles::GL_ELEMENT_ARRAY_BUFFER => self.gl_element_buffer = gl_name,
                    _ => {}
                }
            }
            "BufferData" | "BufferDataQUALCOMM" | "BufferDataATI" => {
                let (target, byte_size, data) = (a[0], a[1], a[2]);
                let Some(gl_name) = self.bound_buffer(target) else {
                    return Ok(Some(SUCCESS));
                };
                // A null `data` is a legitimate request: it reserves the size and leaves the
                // contents undefined. `glBufferSubData` comes later to fill it.
                // The size comes from the game: `glBufferData(target, 0x7fffffff, NULL, usage)`
                // asks for 2 GiB in one line that fits in the guest, and the matching `vec!`
                // aborts the process. The cap is the same as for reads, and for the same reason.
                let content = if data == 0 {
                    vec![0u8; guest_size(byte_size as usize)?]
                } else {
                    self.read_bytes(data, byte_size)?
                };
                self.gl_buffers.insert(gl_name, content);
            }
            "BufferSubData" | "BufferSubDataQUALCOMM" => {
                let (target, start, byte_size, data) = (a[0], a[1], a[2], a[3]);
                let Some(gl_name) = self.bound_buffer(target) else {
                    return Ok(Some(SUCCESS));
                };
                let create = self.read_bytes(data, byte_size)?;
                if let Some(buffer) = self.gl_buffers.get_mut(&gl_name) {
                    let end = start as usize + create.len();
                    // Writing outside the buffer is a game error, and OpenGL says to ignore it
                    // instead of growing: growing would hide the defect and change
                    // `GL_BUFFER_SIZE`.
                    if end <= buffer.len() {
                        buffer[start as usize..end].copy_from_slice(&create);
                    }
                }
            }
            "DeleteBuffers" | "DeleteBuffersQUALCOMM" => {
                for i in 0..a[0] {
                    let gl_name = self.cpu.read_u32(a[1] + i * 4)?;
                    self.gl_buffers.remove(&gl_name);
                    self.buffer_names.free(gl_name);
                    // Deleting a buffer unbinds it, and that is what the specification asks for:
                    // the target goes back to zero, that is, to the game's memory.
                    if self.gl_array_buffer == gl_name {
                        self.gl_array_buffer = 0;
                    }
                    if self.gl_element_buffer == gl_name {
                        self.gl_element_buffer = 0;
                    }
                }
            }
            "IsBuffer" | "IsBufferQUALCOMM" => {
                let exists = u32::from(self.gl_buffers.contains_key(&a[0]));
                answer = Some((1, exists));
            }
            "GetBufferParameteriv" => {
                let (target, pname, output) = (a[0], a[1], a[2]);
                let byte_size = self
                    .bound_buffer(target)
                    .and_then(|gl_name| self.gl_buffers.get(&gl_name))
                    .map_or(0, |buffer| buffer.len() as u32);
                let param_value = match pname {
                    gles::GL_BUFFER_SIZE => byte_size,
                    // The declared usage changes nothing here, and returning what the game asked
                    // for would be making it up: zero is what OpenGL defines for a buffer with no
                    // declared usage.
                    gles::GL_BUFFER_USAGE => 0,
                    _ => 0,
                };
                if output != 0 {
                    self.cpu.write_u32(output, param_value)?;
                }
            }

            // --- Matrices ---------------------------------------------------------------
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

            // --- State ------------------------------------------------------------------
            // **A negative width or height is `GL_INVALID_VALUE`, and the call changes nothing.**
            // Quake 2 computes the level's viewport in 18.14 fixed point and the math overflows at
            // 640×480: it asks for `(0, 68, -179, -67)`. On the console GL refuses it and the
            // previous viewport, the full-screen one, stays in effect; accepted, it sent the whole
            // world off screen and only the HUD showed up.
            "Viewport" if (a[2] as i32) < 0 || (a[3] as i32) < 0 => {}
            "Viewport" => self
                .gl
                .set_viewport(a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32),
            // --- Stencil ------------------------------------------------------------
            //
            // The Z-Wheel stage sets these up twice per frame: it is the planar reflection, which
            // marks the floor in the stencil and draws the mirrored model only where the mark is.
            "StencilFunc" => self.gl.set_stencil_func(a[0], a[1] as i32, a[2]),
            "StencilOp" => self.gl.set_stencil_op(a[0], a[1], a[2]),
            "StencilMask" => self.gl.set_stencil_mask(a[0]),
            "ClearStencil" => self.gl.set_clear_stencil(a[0] as i32),
            "Clear" => {
                if a[0] & gles::GL_COLOR_BUFFER_BIT != 0 {
                    self.gl_clears = self.gl_clears.saturating_add(1);
                }
                // The clear count counts even when skipping — it is a diagnostic of the game, not
                // of the frame the screen showed. Only the actual fill is skipped.
                if !self.skip_draw || self.gl_pixels_read {
                    self.gl.clear(a[0]);
                }
            }
            "ClearColorx" | "ClearColor" => {
                let c = std::array::from_fn(|i| number(a[i]));
                self.gl.set_clear_color(c);
            }
            // `ClearDepth` already takes the depth in `[0, 1]`, which is the buffer's range.
            "ClearDepthx" | "ClearDepthf" => self.gl.set_clear_depth(number(a[0]).clamp(0.0, 1.0)),
            // OpenGL ES 1.1 clamps the current color to [0, 1] when it is set. Alien Breaker
            // Deluxe paints with `glColor4f(255, 255, 255, a)`: without the clamp, the texture was
            // multiplied by 255 and the title and menus blew out to white.
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
                let fixed_point = name.ends_with('x');
                self.gl.set_depth_range(to_scalar(a[0], fixed_point), to_scalar(a[1], fixed_point));
            }
            "AlphaFuncx" | "AlphaFunc" => self.gl.set_alpha_func(a[0], number(a[1])),
            "CullFace" => self.gl.set_cull_face(a[0]),
            "FrontFace" => self.gl.set_front_face(a[0]),
            "TexParameterx" | "TexParameteri" | "TexParameterf" => {
                self.gl.set_texture_parameter(a[1], a[2])
            }
            // The vector forms bring the value through a pointer. Crash asks for `GL_REPLACE`
            // this way, and while only the scalar form was served the mode stayed stuck at
            // `GL_MODULATE`: every texture came out multiplied by the vertex color.
            "TexParameterxv" | "TexParameteriv" | "TexParameterfv" => {
                match a[1] == gles::GL_TEXTURE_CROP_RECT_OES {
                    // The crop is four signed integers, and the sign matters: a negative width
                    // or height mirrors the axis.
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
            // void glDrawTex{sixf}OES(T x, T y, T z, T width, T height) and the vector forms,
            // which bring the five values through a pointer.
            //
            // `s` is a 16-bit integer, `i` a 32-bit one, `x` is 16.16 fixed point and `f` is a
            // float. They all draw the same thing; only how the number arrives changes.
            name if name.starts_with("DrawTex") => self.gles_draw_tex(name, base)?,
            // The mode and the `GL_COMBINE` parameters are enums, and arrive as integers even
            // through the fixed-point or `float` variant; only the scales are numbers. See
            // `TexEnv`.
            "TexEnvx" | "TexEnvi" | "TexEnvf" if a[0] == gles::GL_TEXTURE_ENV => {
                let num_value = match name {
                    "TexEnvx" => gles::fixed(a[2]),
                    "TexEnvf" => f32::from_bits(a[2]),
                    _ => a[2] as i32 as f32,
                };
                let enum_value = match name {
                    "TexEnvf" => f32::from_bits(a[2]) as u32,
                    _ => a[2],
                };
                match a[1] {
                    gles::GL_TEXTURE_ENV_MODE => self.gl.set_texture_env(enum_value),
                    pname => self.gl.set_texture_env_param(pname, enum_value, num_value),
                }
            }
            "TexEnvxv" | "TexEnviv" | "TexEnvfv" if a[0] == gles::GL_TEXTURE_ENV => {
                match a[1] {
                    gles::GL_TEXTURE_ENV_COLOR => {
                        let mut color = [0.0f32; 4];
                        for (i, channel) in color.iter_mut().enumerate() {
                            let raw = self.cpu.read_u32(a[2] + i as u32 * 4)?;
                            *channel = match name {
                                "TexEnvxv" => gles::fixed(raw),
                                "TexEnvfv" => f32::from_bits(raw),
                                // Integers go from 0 to the largest positive, like every integer
                                // color.
                                _ => (raw as i32 as f32 / i32::MAX as f32).max(0.0),
                            };
                        }
                        self.gl.set_texture_env_color(color);
                    }
                    gles::GL_TEXTURE_ENV_MODE => {
                        let value = self.cpu.read_u32(a[2])?;
                        self.gl.set_texture_env(value);
                    }
                    pname => {
                        let raw = self.cpu.read_u32(a[2])?;
                        let (enum_value, num_value) = match name {
                            "TexEnvxv" => (raw, gles::fixed(raw)),
                            "TexEnvfv" => (f32::from_bits(raw) as u32, f32::from_bits(raw)),
                            _ => (raw, raw as i32 as f32),
                        };
                        self.gl.set_texture_env_param(pname, enum_value, num_value);
                    }
                }
            }
            // The point environment (`GL_POINT_SPRITE_OES`) has nothing we draw.
            "TexEnvx" | "TexEnvi" | "TexEnvf" | "TexEnvxv" | "TexEnviv" | "TexEnvfv" => {}
            "ActiveTexture" => self.gl.set_active_texture(a[0]),
            "ClientActiveTexture" => self.gl.set_client_active_texture(a[0]),
            "BindTexture" => {
                self.texture_names.mark_used(a[1]);
                self.gl.bind_texture(a[1]);
            }
            "TexImage2D" => self.gles_tex_image(&a)?,
            "TexSubImage2D" => self.gles_tex_sub_image(&a)?,
            "CompressedTexImage2D" => self.gles_compressed_tex_image(&a)?,

            // --- Fixed-function lighting ------------------------------------------------
            //
            // The Z-Wheel stage depends on it: it enables `GL_LIGHTING` and `GL_LIGHT0`, sets the
            // light's ambient to 0.5 and the ambient and diffuse material to 0.949, and leaves
            // everything else at the default — including light zero's white diffuse and the
            // position `(0, 0, 1, 0)`, which is directional. Without any of that, the models came
            // out with the raw vertex color.
            "Lightxv" | "Lightfv" => {
                let light = a[0].wrapping_sub(gles::GL_LIGHT0) as usize;
                let param_values = self.read_param(a[1], a[2], name.ends_with("xv"))?;
                self.gl.set_light(light, a[1], param_values);
            }
            "Materialxv" | "Materialfv" => {
                let param_values = self.read_param(a[1], a[2], name.ends_with("xv"))?;
                self.gl.set_material(a[1], param_values);
            }
            "LightModelxv" | "LightModelfv" => {
                let param_values = self.read_param(a[0], a[1], name.ends_with("xv"))?;
                self.gl.set_light_model(a[0], param_values);
            }
            // The scalar forms bring the value in the argument itself.
            "Lightx" | "Lightf" => {
                let light = a[0].wrapping_sub(gles::GL_LIGHT0) as usize;
                let param_value = to_scalar(a[2], name.ends_with('x'));
                self.gl.set_light(light, a[1], [param_value, 0.0, 0.0, 0.0]);
            }
            "Materialx" | "Materialf" => {
                let param_value = to_scalar(a[2], name.ends_with('x'));
                self.gl.set_material(a[1], [param_value, 0.0, 0.0, 0.0]);
            }
            "LightModelx" | "LightModelf" => {
                let param_value = to_scalar(a[1], name.ends_with('x'));
                self.gl.set_light_model(a[0], [param_value, 0.0, 0.0, 0.0]);
            }
            // Fog. `GL_FOG_MODE` arrives as a number — `GL_LINEAR`, `GL_EXP` or `GL_EXP2` —, and
            // in the `x` forms it comes as an **integer**, not fixed point: it is an enumeration,
            // and converting it as a scale would give `0x2601/65536`, which is no mode at all.
            "Fogxv" | "Fogfv" => {
                let param_values = match a[0] == gles::GL_FOG_MODE {
                    true => [self.cpu.read_u32(a[1])? as f32, 0.0, 0.0, 0.0],
                    false => self.read_param(a[0], a[1], name.ends_with("xv"))?,
                };
                self.gl.set_fog(a[0], param_values);
            }
            "Fogx" | "Fogf" => {
                let param_value = match a[0] == gles::GL_FOG_MODE {
                    true => a[1] as f32,
                    false => to_scalar(a[1], name.ends_with('x')),
                };
                self.gl.set_fog(a[0], [param_value, 0.0, 0.0, 0.0]);
            }
            "ShadeModel" => self.gl.set_shade_model(a[0]),
            "Normal3x" | "Normal3f" => {
                let fixed_point = name.ends_with('x');
                self.gl_current_normal = std::array::from_fn(|i| to_scalar(a[i], fixed_point));
            }
            // glNormalPointer(type, stride, pointer) — **no size**: a normal always has three.
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

            // --- Arrays and drawing -----------------------------------------------------
            "VertexPointer" | "ColorPointer" | "TexCoordPointer" => {
                let pointer = ArrayPointer {
                    size: a[0],
                    kind: a[1],
                    stride: a[2],
                    address: a[3],
                    enabled: true,
                    buffer: self.gl_array_buffer,
                };
                // The coordinate array belongs to the unit chosen by `glClientActiveTexture`;
                // only the first two draw.
                let slot = match (name, self.gl.client_unit()) {
                    ("VertexPointer", _) => &mut self.gl_vertices,
                    ("ColorPointer", _) => &mut self.gl_colors,
                    (_, 0) => &mut self.gl_texcoords,
                    (_, 1) => &mut self.gl_texcoords1,
                    _ => return Ok(Some(SUCCESS)),
                };
                // `enabled` belongs to `EnableClientState`, not to the pointer: changing the
                // pointer neither enables nor disables the array.
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
                // The whole list in a single request, for the same reason as `read_array`: each
                // trip to the backend costs more than the two bytes it brings.
                let width_u32 = if kind == gles::GL_UNSIGNED_BYTE { 1 } else { 2 };
                // With an index buffer bound, `list` is an offset inside it — and here the
                // **current** binding applies, unlike the vertex arrays.
                let bytes =
                    self.array_bytes(self.gl_element_buffer, list, count * width_u32)?;
                let indices: Vec<u32> = bytes
                    .chunks_exact(width_u32 as usize)
                    .map(|c| {
                        if width_u32 == 1 {
                            c[0] as u32
                        } else {
                            u16::from_le_bytes([c[0], c[1]]) as u32
                        }
                    })
                    .collect();
                self.gles_draw(mode, &indices)?;
            }

            "GetIntegerv" | "GetFixedv" | "GetBooleanv" => {
                let (width_u32, height_u32) = self.gl.frame_size();
                let (width, height) = (width_u32 as i32, height_u32 as i32);
                let values = gles::integer(a[0], width, height).unwrap_or(&[0]);
                for (i, &value) in values.iter().enumerate() {
                    if a[1] != 0 {
                        self.cpu.write_u32(a[1] + i as u32 * 4, value as u32)?;
                    }
                }
            }
            // The other `Get*v` write to the pointer in the second argument; zeroing is better
            // than leaving garbage, and no game depends on them yet.
            name if name.starts_with("Get") => self.write_at(a[1], 0)?,
            // The rest is served successfully and does nothing. That is deliberate for the state
            // our rasterizer does not use — depth, fog, light —, and refusing would bring games
            // down for nothing. But the silence hides the ones that **change the drawing**:
            // `TexSubImage2D` was here, and the effect was a scrambled texture without a single
            // line of warning. Logging costs nothing, and gives a place to start investigating a
            // wrong drawing.
            // glReadPixels(x, y, width, height, format, type, pixels)
            //
            // It is how the game takes the character's photo: it draws and reads the frame back.
            // While this didn't exist, it read whatever was in its buffer — hence the scrambled
            // image.
            "ReadPixels" => self.gles_read_pixels(&a)?,
            // glScissor(x, y, width, height) — the rectangle outside of which nothing is drawn,
            // with `y` going bottom to top, like the viewport.
            //
            // Peggle draws the whole font sheet and narrows the scissor so a single letter shows.
            // Ignored, the whole sheet went to the screen on top of the game.
            // Same rule as `Viewport`: a negative size is refused without changing the state.
            "Scissor" if (a[2] as i32) < 0 || (a[3] as i32) < 0 => {}
            "Scissor" => self
                .gl
                .set_scissor(a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32),
            // glPixelStorei(pname, param) — for now only the row alignment for texture upload.
            // **It is the only GL call the survey of the 62 ROMs caught as "served without doing
            // anything"**: Double Dragon, Galaxy on Fire, Pac-Mania, Powerboat Challenge and
            // Raging Thunder 2 use it, and ignoring it shifts the rows of a texture whose width in
            // bytes is not a multiple of four — the image comes out scrambled diagonally, without a
            // line of warning anywhere.
            "PixelStorei" => {
                if a[0] == gles::GL_UNPACK_ALIGNMENT {
                    self.unpack_alignment = match a[1] {
                        // Only these four values are legal in GL; any other is a game error, and
                        // keeping what was there is safer than starting to read skewed.
                        1 | 2 | 4 | 8 => a[1],
                        _ => self.unpack_alignment,
                    };
                }
            }
            // glColorMask(r, g, b, a) — booleans, one per channel.
            "ColorMask" => self.gl.set_color_mask(std::array::from_fn(|i| a[i] != 0)),
            other => {
                if !SILENTLY_HANDLED.contains(&other) {
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

    /// Reads the sixteen numbers of a matrix from guest memory.
    pub(super) fn read_matrix(
        &self,
        address: u32,
        fixed_point: bool,
    ) -> Result<rasterizer::Matrix, CpuError> {
        // All sixteen at once: it used to be sixteen trips to the backend on every
        // `LoadMatrix`/`MultMatrix`, and the matrix is contiguous by definition. If any part of
        // it is outside the map, the read fails — as it failed before, on the first bad
        // component.
        let mut bytes = [0u8; 64];
        self.cpu.read_mem(address, &mut bytes)?;
        let mut m = rasterizer::IDENTITY;
        for (slot, word_bytes) in m.iter_mut().zip(bytes.chunks_exact(4)) {
            let word = u32::from_le_bytes([word_bytes[0], word_bytes[1], word_bytes[2], word_bytes[3]]);
            *slot = if fixed_point {
                gles::fixed(word)
            } else {
                f32::from_bits(word)
            };
        }
        Ok(m)
    }

    /// `TexImage2D(target, level, internalformat, width, height, border, format, type,
    /// pixels)` — nine arguments, most of which arrive on the stack.
    /// `glCompressedTexImage2D(target, level, internalformat, width, height, border,
    /// imageSize, data)`.
    ///
    /// Only the ATITC formats, which are the Adreno 130's and the only ones the console's games
    /// use — Boomerang Sports Dodgeball loads all its textures this way.
    pub(super) fn gles_compressed_tex_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, format, width, height) = (a[1], a[2], a[3], a[4]);
        let (size, pixels) = (a[6], a[7]);
        // The OES paletted textures come in through the same path. Their `level` is
        // non-positive — zero is just the base level, and a negative one says how many mipmaps
        // follow —, so the level check below does not apply to them.
        if let Some(palette) = paltex::Format::from_gl(format) {
            if width == 0 || height == 0 || pixels == 0 {
                return Ok(());
            }
            let bytes = self.read_bytes(pixels, size)?;
            let Some(decoded) = paltex::decode(&bytes, width as usize, height as usize, palette)
            else {
                self.note_bad_pointer(format!(
                    "paletted texture {format:#x} without a complete palette"
                ));
                return Ok(());
            };
            let name = self.gl.bound_texture();
            // The paletted one brings the whole chain in a single block, and its `level` counts
            // the mipmaps instead of naming them; the decoder returns the base level.
            self.gl
                .upload_level(name, 0, width as usize, height as usize, decoded);
            return Ok(());
        }
        let explicit_alpha = match format {
            gles::GL_ATC_RGB_AMD => false,
            gles::GL_ATC_RGBA_EXPLICIT_ALPHA_AMD => true,
            _ => {
                self.note_bad_pointer(format!("compressed texture in format {format:#x}"));
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
            let bytes = self.read_texels(pixels, width, height, format, kind)?;
            decode_texels(&bytes, format, kind, texels)
        };
        // The wrap and filter parameters survive a new image: in OpenGL they belong to the
        // texture name, not the content, and the game usually sets them only once.
        let name = self.gl.bound_texture();
        self.gl
            .upload_level(name, level, width as usize, height as usize, decoded);
        Ok(())
    }

    /// `glReadPixels`: copies a rectangle of the frame to the game's memory.
    ///
    /// We serve the two formats OpenGL ES 1.1 mandates: `RGBA` with eight bits per channel and
    /// `RGB` in 565. Any other is logged instead of written, because filling with the wrong
    /// format gives a plausible and false image — worse than not writing.
    pub(super) fn gles_read_pixels(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (x, y) = (a[0] as i32, a[1] as i32);
        let (width, height) = (a[2] as usize, a[3] as usize);
        let (format, kind, dest) = (a[4], a[5], a[6]);
        if width == 0 || height == 0 || dest == 0 {
            return Ok(());
        }
        // From here on frameskip can no longer skip draw/clear: this game observes the
        // framebuffer, and handing over the previous image stops being a visual loss and becomes
        // wrong data in guest memory.
        self.gl_pixels_read = true;
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
                self.note_bad_pointer(format!(
                    "ReadPixels in format {format:#x}/{kind:#x}, which we don't know how to write"
                ));
                return Ok(());
            }
        };
        self.cpu.write_mem(dest, &bytes)?;
        Ok(())
    }

    /// `glTexSubImage2D`: replaces a rectangle inside a texture that already exists.
    ///
    /// It is how images are built in pieces — a portrait inside an atlas, a number that changes —,
    /// and while this didn't exist the call was served silently: the texture kept the old content
    /// and the drawing came out scrambled.
    ///
    /// If the rectangle does not fit in the texture, we write nothing. Clipping would be making up
    /// a result OpenGL does not define.
    /// Reads the texels of a guest image, honoring the row alignment.
    ///
    /// `glPixelStorei(GL_UNPACK_ALIGNMENT, n)` says on how many bytes each row starts aligned in
    /// the game's memory — the OpenGL default is 4. Without this, a texture with an odd width in
    /// bytes arrives with its rows shifted pair by pair: the image comes out scrambled diagonally,
    /// without warning.
    fn read_texels(
        &mut self,
        pixels: u32,
        width: u32,
        height: u32,
        format: u32,
        kind: u32,
    ) -> Result<Vec<u8>, CpuError> {
        let per_texel = bytes_per_texel(format, kind);
        let pressed = width * per_texel;
        let step = round_up_to(pressed, self.unpack_alignment);
        if step == pressed {
            return self.read_bytes(pixels, pressed * height);
        }
        // **A single read**, of the whole aligned block, and the rows are packed afterwards:
        // asking row by row is `height` trips to guest memory, and each trip costs more than the
        // bytes it brings.
        let block = self.read_bytes(pixels, step * height)?;
        let mut output = Vec::with_capacity((pressed * height) as usize);
        for line in 0..height as usize {
            let start = line * step as usize;
            output.extend_from_slice(&block[start..start + pressed as usize]);
        }
        Ok(output)
    }

    pub(super) fn gles_tex_sub_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, x, y, width, height) = (a[1], a[2], a[3], a[4], a[5]);
        let (format, kind, pixels) = (a[6], a[7], a[8]);
        if level != 0 || width == 0 || height == 0 || pixels == 0 {
            return Ok(());
        }
        let texels = (width * height) as usize;
        let bytes = self.read_texels(pixels, width, height, format, kind)?;
        let fresh = decode_texels(&bytes, format, kind, texels);

        let name = self.gl.bound_texture();
        if let Err(Some((tw, th))) = self.gl.sub_image(name, x, y, width, height, &fresh) {
            self.note_bad_pointer(format!(
                "TexSubImage2D of {width}x{height} at ({x},{y}) does not fit in a {tw}x{th} texture"
            ));
        }
        Ok(())
    }

    /// Builds the vertices from the client arrays and sends them to be drawn.
    /// `glDrawTex{sixf}[v]OES`: a rectangle of the texture, cropped by `GL_TEXTURE_CROP_RECT_OES`,
    /// straight in screen coordinates.
    ///
    /// `base` is where the first argument is: 1 when the method belongs to an object (`IGLES11`
    /// and `IGLES11Ext`, with the object in `r0`), 0 in legacy GL. `IGLES11Ext` answered "done"
    /// without drawing, and Ridge Racer builds its whole menu and HUD through it: only the blue
    /// background was left.
    pub(super) fn gles_draw_tex(&mut self, name: &str, base: usize) -> Result<(), CpuError> {
        let vector = name.ends_with("vOES");
        let scale = match name.as_bytes().get(7) {
            Some(b'x') => 1.0 / 65536.0,
            _ => 1.0,
        };
        let float = name.as_bytes().get(7) == Some(&b'f');
        let mut values = [0f32; 5];
        for (index, slot) in values.iter_mut().enumerate() {
            let raw = match vector {
                true => self.cpu.read_u32(self.arg(base) + index as u32 * 4)?,
                false => self.arg(base + index),
            };
            *slot = match float {
                true => f32::from_bits(raw),
                false => raw as i32 as f32 * scale,
            };
        }
        let [x, y, z, width, height] = values;
        self.gl.draw_texture(x, y, z, width, height);
        Ok(())
    }

    pub(super) fn gles_draw(&mut self, mode: u32, indices: &[u32]) -> Result<(), CpuError> {
        if !self.gl_vertices.in_use() || indices.is_empty() {
            return Ok(());
        }
        // **A skipped frame leaves here, before any read of guest memory.** That is what makes
        // skipping really save something: without this, the expensive cost — crossing the unicorn
        // FFI to bring each vertex — would happen anyway, and only the rasterization would go
        // away. The game sees no difference at all: real hardware doesn't say whether the pixel
        // reached the screen either.
        if self.skip_draw && !self.gl_pixels_read {
            return Ok(());
        }
        let base = self.gl.current_color();
        // One block per array, not one per component: it is the same guest memory, asked for at
        // once. See [`Self::read_array`].
        let positions = self.read_array(self.gl_vertices, indices, [0.0, 0.0, 0.0, 1.0])?;
        let colors = self
            .gl_colors
            .in_use()
            .then(|| self.read_array(self.gl_colors, indices, [0.0, 0.0, 0.0, 1.0]))
            .transpose()?;
        let uvs = self
            .gl_texcoords
            .in_use()
            .then(|| self.read_array(self.gl_texcoords, indices, rasterizer::DEFAULT_UV))
            .transpose()?;
        let uvs1 = self
            .gl_texcoords1
            .in_use()
            .then(|| self.read_array(self.gl_texcoords1, indices, rasterizer::DEFAULT_UV))
            .transpose()?;
        let normals = self
            .gl_normals
            .in_use()
            .then(|| self.read_array(self.gl_normals, indices, [0.0, 0.0, 1.0, 0.0]))
            .transpose()?;
        let vertices: Vec<Vertex> = (0..indices.len())
            .map(|i| Vertex {
                position: positions[i],
                color: colors.as_ref().map_or(base, |c| c[i]),
                uv: uvs.as_ref().map_or(rasterizer::DEFAULT_UV, |t| t[i]),
                uv1: uvs1.as_ref().map_or(rasterizer::DEFAULT_UV, |t| t[i]),
                normal: normals
                    .as_ref()
                    .map_or(self.gl_current_normal, |n| [n[i][0], n[i][1], n[i][2]]),
                // The fog factor is computed by the vertex stage, which is where the distance in
                // eye coordinates exists.
                fog: 1.0,
            })
            .collect();
        self.gl.draw(mode, &vertices);
        Ok(())
    }

    /// Reads the components of a light or material parameter from the game's pointer.
    ///
    /// How many to read comes from the parameter itself — see [`gles::components`] —, not always
    /// four: `GL_SHININESS` has only one, and reading four runs over whatever comes after it on
    /// the game's stack.
    pub(super) fn read_param(
        &self,
        pname: u32,
        pointer: u32,
        fixed_point: bool,
    ) -> Result<[f32; 4], CpuError> {
        let mut values = [0.0f32; 4];
        let count = gles::components(pname).min(4);
        if count == 0 {
            return Ok(values);
        }
        // A single request for the up to four components, for the same reason as `read_array`.
        let mut bytes = [0u8; 16];
        self.cpu.read_mem(pointer, &mut bytes[..count * 4])?;
        for (value, word) in values.iter_mut().zip(bytes.chunks_exact(4)).take(count) {
            *value = to_scalar(
                u32::from_le_bytes([word[0], word[1], word[2], word[3]]),
                fixed_point,
            );
        }
        Ok(values)
    }

    /// Reads one element of a client array, filling in the missing components.
    /// Reads at once the span of the array the indices cover, and decodes from there.
    ///
    /// The per-component path crosses the backend to copy four bytes, and it looks up the region
    /// before copying: **57 ns**, against **0.3 ns** when the same four bytes come from a
    /// one-kilobyte `read_mem`. In Quake there are 6.6 million vertices in 15 virtual seconds, each
    /// with position and texture coordinate — and that was **3.2 s of the 3.6 s** the draw calls
    /// cost, against 400 ms for the actual rasterizer.
    ///
    /// If the block read fails, it falls back to the old path. The span goes from the first to the
    /// last index, and an array that ends flush against the end of the mapped region may have the
    /// last element's `stride` padding outside it — what held before still holds.
    /// Which name is bound to a buffer target. `None` when it is zero — which is no buffer at
    /// all, but "the pointers are addresses in the game's memory".
    pub(super) fn bound_buffer(&self, target: u32) -> Option<u32> {
        let name = match target {
            gles::GL_ARRAY_BUFFER => self.gl_array_buffer,
            gles::GL_ELEMENT_ARRAY_BUFFER => self.gl_element_buffer,
            _ => 0,
        };
        (name != 0).then_some(name)
    }

    /// Bytes of an array, whether they come from the buffer object or from the game's memory.
    ///
    /// This is the only place that knows the difference, and on purpose: with a buffer bound, the
    /// "pointer" the game passed is no address at all — it is an offset inside the buffer.
    /// Reading the game's memory at that number would give garbage or an access fault, and that is
    /// what `glVertexPointer(…, 0)` from a game with a bound buffer would do: read address zero.
    fn array_bytes(
        &self,
        buffer: u32,
        address: u32,
        count: u32,
    ) -> Result<Vec<u8>, CpuError> {
        if buffer == 0 {
            return self.read_bytes(address, count);
        }
        let empty = Vec::new();
        let content = self.gl_buffers.get(&buffer).unwrap_or(&empty);
        Ok(buffer_slice(content, address, count))
    }

    pub(super) fn read_array(
        &self,
        pointer: ArrayPointer,
        indices: &[u32],
        default: [f32; 4],
    ) -> Result<Vec<[f32; 4]>, CpuError> {
        let loose = |machine: &Self| -> Result<Vec<[f32; 4]>, CpuError> {
            indices
                .iter()
                .map(|&i| machine.read_attribute(pointer, i, default))
                .collect()
        };
        let component = component_size(pointer.kind);
        let width = pointer.size.min(4) * component;
        let stride = if pointer.stride == 0 {
            pointer.size * component
        } else {
            pointer.stride
        };
        let (Some(&smallest), Some(&largest)) = (indices.iter().min(), indices.iter().max()) else {
            return Ok(Vec::new());
        };
        // In u64 because `largest * stride` overflows u32 with a corrupted pointer, and then the
        // cap below is precisely what needs to see the big number to refuse it.
        let extension = (largest - smallest) as u64 * stride as u64 + width as u64;
        let start = pointer.address as u64 + smallest as u64 * stride as u64;
        if extension > ARRAY_CAP || start + extension > u32::MAX as u64 {
            return loose(self);
        }
        let Ok(bytes) = self.array_bytes(pointer.buffer, start as u32, extension as u32)
        else {
            return loose(self);
        };
        Ok(indices
            .iter()
            .map(|&index| {
                let base = (index - smallest) as usize * stride as usize;
                let mut out = default;
                for i in 0..pointer.size.min(4) as usize {
                    let at = base + i * component as usize;
                    out[i] = match pointer.kind {
                        gles::GL_FLOAT => f32::from_le_bytes([
                            bytes[at],
                            bytes[at + 1],
                            bytes[at + 2],
                            bytes[at + 3],
                        ]),
                        gles::GL_FIXED => gles::fixed(u32::from_le_bytes([
                            bytes[at],
                            bytes[at + 1],
                            bytes[at + 2],
                            bytes[at + 3],
                        ])),
                        gles::GL_SHORT => i16::from_le_bytes([bytes[at], bytes[at + 1]]) as f32,
                        gles::GL_UNSIGNED_SHORT => {
                            u16::from_le_bytes([bytes[at], bytes[at + 1]]) as f32
                        }
                        gles::GL_BYTE => bytes[at] as i8 as f32,
                        // `GL_UNSIGNED_BYTE` only shows up in color, and there the value is
                        // normalized.
                        _ => bytes[at] as f32 / 255.0,
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
        let count = pointer.size.min(4);
        // A single request for the whole element, instead of one per component: it is the same
        // reason as `read_array`, and now also what lets the buffer come in through one path.
        let bytes = self.array_bytes(
            pointer.buffer,
            pointer.address + index * stride,
            count * component,
        )?;
        let mut out = default;
        for i in 0..count as usize {
            let at = i * component as usize;
            let Some(field) = bytes.get(at..at + component as usize) else {
                break;
            };
            out[i] = match pointer.kind {
                gles::GL_FLOAT => f32::from_le_bytes([field[0], field[1], field[2], field[3]]),
                gles::GL_FIXED => gles::fixed(u32::from_le_bytes([
                    field[0], field[1], field[2], field[3],
                ])),
                gles::GL_SHORT => i16::from_le_bytes([field[0], field[1]]) as f32,
                gles::GL_UNSIGNED_SHORT => u16::from_le_bytes([field[0], field[1]]) as f32,
                gles::GL_BYTE => field[0] as i8 as f32,
                // `GL_UNSIGNED_BYTE` only shows up in color, and there the value is normalized.
                _ => field[0] as f32 / 255.0,
            };
        }
        Ok(out)
    }

    /// The Nth AAPCS argument: `r0..r3` and, from there on, stack words.
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

    /// Closes the frame on `eglSwapBuffers`.
    ///
    /// **Paints the queue and does not read the result.** These are two different things, and
    /// separating them is the gain:
    ///
    /// - painting the queue is the frame's **rasterization**, and it cannot be deferred — it is the
    ///   work the game asked for;
    /// - reading the frame back to CPU memory exists for **2D drawing on top** and for when the
    ///   guest asks for the pixels. A pure 3D game does neither, and on the handheld that read
    ///   forces the tiled GPU to finish and hand back the frame on every swap.
    ///
    /// So the frame stays **pending**, and [`Machine::materialize_gl_frame`] brings it when someone
    /// really needs it. The drawing path decides: any 2D call materializes before writing, because
    /// writing over a stale frame would erase the scene.
    pub(super) fn present_gl(&mut self) {
        self.gl.flush_draw();
        self.gl.end_frame();
        self.gl_frame_pending = true;
        self.gpu_without_frame = false;
        // **The frame on the GPU stands for the screen as it is now**, even without having been
        // read. It is what [`Machine::gpu_frame`] compares, and whoever presents through the GPU
        // never materializes — the texture is precisely what makes the read unnecessary. Marked
        // only on materialization, the 3D frame was never considered intact on this path: the
        // window fell back to the CPU screen, which hadn't received the frame, and the game came
        // out black in every window that presents through the GPU — measured with Double Dragon,
        // which draws its title through OpenGL. A 2D drawing after this materializes before
        // writing, and the write undoes the mark, as it should.
        self.gl_frame_writes = Some(self.screen().writes());
        // **Only the GPU defers.** In the CPU rasterizer the read is an in-memory conversion:
        // there is no wait to save, and the frontend reads the screen every frame anyway —
        // deferring there would only create the chance of it presenting a stale frame.
        if !self.gl.frame_waits_for_gpu() {
            self.materialize_gl_frame();
        }
    }

    /// State calls sent to the GPU and how many the mirror spared. See
    /// [`crate::video::gpu::Mirror`].
    pub fn state_sent_and_spared(&self) -> (u64, u64) {
        self.gl.state_sent_and_spared()
    }

    /// How many times the GPU frame was brought to the CPU screen in this session.
    ///
    /// Compared with [`Machine::gl_swaps`] it says how much the deferral paid off: each buffer
    /// swap without materialization is a frame read that did **not** happen. It serves as a check
    /// number on the handheld, where the read is the expensive part: see [`Machine::present_gl`].
    pub fn gl_frame_materializations(&self) -> u32 {
        self.gl_materializations
    }

    /// Brings to the CPU screen the frame `eglSwapBuffers` left pending.
    ///
    /// Called by every path that **reads or writes** the console screen: the three 2D drawing
    /// interfaces, the pixel read and the diagnostic dump. It is not called by the window or by the
    /// core when they present the GPU texture — those don't want the pixels, they want the texture,
    /// and the readback existed for them by mistake.
    pub fn materialize_gl_frame(&mut self) {
        if !self.gl_frame_pending {
            return;
        }
        self.gl_frame_pending = false;
        self.gl_materializations = self.gl_materializations.saturating_add(1);
        // The screen may be the "device bitmap" surface, when the game asked for one — that is
        // the one that counts, not the fallback framebuffer.
        let (width, height) = {
            let target = self.screen();
            (target.width() as usize, target.height() as usize)
        };
        // The frame goes straight into the previous one's buffer, in RGB565: without the `u16`
        // vector and the round trip to bytes, and with no conversion at all when nothing was drawn
        // since the last one.
        let mut words = std::mem::take(&mut self.gl_last_frame_words);
        self.gl.frame_rgb565_words(width, height, &mut words);
        match self.bitmaps.get_mut(&self.device_bitmap) {
            Some(surface) => surface.load_rgb565_words(&words),
            None => self.screen.load_rgb565_words(&words),
        }
        self.gl_last_frame_words = words;
        // **The mark is from the instant the screen became equal to the 3D frame**, and that is
        // why it comes here and not at the buffer swap: `gpu_frame` compares this count with the
        // current one to know whether any 2D drew on top after that.
        self.gl_frame_writes = Some(self.screen().writes());
    }

    /// The frame texture, for a frontend that presents on its own GPU.
    pub fn frame_texture(&self) -> Option<u32> {
        self.gl.frame_texture()
    }

    /// The 3D frame at the internal resolution, when it is the one on screen.
    ///
    /// Only valid while the screen is exactly what the last `eglSwapBuffers` put there: any 2D
    /// drawing after that — a HUD through `IDisplay`, a message box — lives only on the console
    /// screen, and showing the large texture would erase it. In that case the window keeps the
    /// 640×480 screen, as always.
    ///
    /// **Not right after restoring a state either.** The GPU texture is not part of the save state,
    /// and comes back empty; the CPU screen comes back with the saved image. Since the screen's
    /// write counter comes back with it, the screen looked intact, and the window showed the
    /// texture: black. In rewind, which holds the game still at each point, the screen stayed black
    /// from start to finish — measured on Magical Drop III and Um Jogo de Ovos, through the GPU.
    /// Until the next buffer swap the screen applies.
    pub fn gpu_frame(&self) -> Option<crate::video::rasterizer::GpuFrame> {
        let intact = !self.gpu_without_frame
            && self.gl_frame_writes == Some(self.screen().writes());
        intact.then(|| self.gl.gpu_frame()).flatten()
    }

    /// The 3D frame at the internal resolution, as a surface, for recording without a window.
    pub fn large_frame(&mut self) -> Option<Framebuffer> {
        let (w, h, rgba) = self.large_frame_rgba()?;
        let mut frame = Framebuffer::new(w as u32, h as u32);
        for (i, p) in rgba.chunks_exact(4).enumerate() {
            let color = Rgb { r: p[0], g: p[1], b: p[2] };
            frame.set_pixel((i % w) as i32, (i / w) as i32, color);
        }
        Some(frame)
    }

    /// The 3D frame at the internal resolution as the GPU has it: eight-bit RGBA, rows top to
    /// bottom. [`Machine::large_frame`] turns it into RGB565, and that drops bits the GPU had
    /// already computed; the screenshot wants all eight.
    pub fn large_frame_rgba(&mut self) -> Option<(usize, usize, Vec<u8>)> {
        self.gl.read_large_frame()
    }

    /// The internal resolution of the GPU rasterizer. See [`Rasterizer::set_scale`].
    pub fn set_internal_resolution(&mut self, scale: usize) {
        self.gl.set_scale(scale);
    }

    /// Makes drawing go to the frontend's framebuffer, when it provides one.
    ///
    /// It is what `libretro` asks of whoever uses hardware rendering: the core draws to the
    /// framebuffer the frontend points to each frame, and the frontend presents it. Without an
    /// outside framebuffer, the engine draws to its own and the frame comes out through
    /// `frame_rgb565`, as always.
    pub fn draw_to_fbo(&mut self, fbo: Option<u32>) {
        self.gl.draw_to_fbo(fbo);
    }

    /// The window painted on the borrowed context. See [`Rasterizer::resume_context`].
    pub fn resume_context(&mut self) {
        self.gl.resume_context();
    }

    /// The frame is over: the context goes back to the frontend. See
    /// [`Rasterizer::hand_back_to_frontend`].
    pub fn hand_back_to_frontend(&mut self) {
        self.gl.hand_back_to_frontend();
    }

    /// Tells the GPU rasterizer to discard depth and stencil after the frame.
    /// See [`Rasterizer::set_tile_discard`].
    pub fn set_tile_discard(&mut self, discard: bool) {
        self.gl.set_tile_discard(discard);
    }

    /// Lowers the internal 3D resolution in the CPU rasterizer. See
    /// [`Rasterizer::set_downscale`].
    pub fn set_downscale(&mut self, downscale: usize) {
        self.gl.set_downscale(downscale);
    }

    /// The experimental 3D aspect ratio. See [`Rasterizer::set_aspect_ratio`].
    pub fn set_aspect_ratio(&mut self, aspect: Option<f32>) {
        self.gl.set_aspect_ratio(aspect);
    }

    /// The GPU rasterizer's image enhancements: antialiasing and anisotropic filtering. See
    /// [`Rasterizer::set_antialias`] and [`Rasterizer::set_anisotropic`].
    pub fn set_enhancements(&mut self, samples: usize, anisotropic: usize) {
        self.gl.set_antialias(samples);
        self.gl.set_anisotropic(anisotropic);
    }

    /// Whether the game's fog applies. The player's choice, not the game's — see
    /// [`rasterizer::Rasterizer::set_fog_allowed`].
    pub fn set_fog_allowed(&mut self, allowed: bool) {
        self.gl.set_fog_allowed(allowed);
    }

    /// Whether the current frame should skip drawing — see the `skip_draw` field.
    ///
    /// Called once per frame, before the game runs: the decision applies to every `gles_draw` and
    /// `Clear` that happen while the CPU emulates this frame, and is re-evaluated on the next.
    pub fn set_skip_draw(&mut self, skip: bool) {
        self.skip_draw = skip;
    }

    /// Whether the game read pixels from the framebuffer and therefore disabled rasterization
    /// frameskip.
    pub fn did_read_pixels(&self) -> bool {
        self.gl_pixels_read
    }

    /// Hands back to its owner the GL state the rasterizer touched. See
    /// [`rasterizer::Rasterizer::hand_back_context`].
    pub fn hand_back_context(&self) {
        self.gl.hand_back_context();
    }
}

/// The slice of a buffer object an array asks for, or zeros when it doesn't fit.
///
/// Kept apart from the machine because it is the only rule of the buffer path that can go wrong
/// on its own — and because it can be checked in a test without bringing up a whole ARM core.
///
/// Out of the buffer is **not** a reason to drop the drawing: OpenGL leaves the result undefined,
/// and zero is the most harmless undefined one can pick. Returning an error here would abort the
/// whole `glDrawElements` because of one vertex, and a game that uploads its mesh in pieces goes
/// through this case without being defective.
fn buffer_slice(content: &[u8], address: u32, count: u32) -> Vec<u8> {
    let start = address as usize;
    let end = start.saturating_add(count as usize);
    match content.get(start..end) {
        Some(slice) => slice.to_vec(),
        None => vec![0u8; count as usize],
    }
}

#[cfg(test)]
mod name_tests {
    use super::GlNames;

    /// A deleted name comes out again, and the smallest free one comes out. Without this Rally
    /// Master Pro went past 248 textures in a few races: see [`GlNames`].
    #[test]
    fn a_deleted_name_is_handed_out_again() {
        let mut names = GlNames::default();
        assert_eq!([names.generate(), names.generate(), names.generate()], [1, 2, 3]);
        names.free(2);
        assert_eq!(names.generate(), 2);
        assert_eq!(names.generate(), 4);
    }

    /// A thousand cycles of deleting and recreating the same scene don't make the name grow.
    #[test]
    fn recreating_the_scene_does_not_grow_the_name() {
        let mut names = GlNames::default();
        let fixed_point = names.generate();
        for _ in 0..1000 {
            let scene: Vec<u32> = (0..60).map(|_| names.generate()).collect();
            for name in scene {
                names.free(name);
            }
        }
        assert_eq!(names.largest(), fixed_point);
        assert_eq!(names.generate(), 2);
    }

    /// A name bound without having been generated comes into existence, and `glGen*` does not
    /// hand it to anyone else. Zero is the default object and never goes in.
    #[test]
    fn a_name_bound_without_gen_stays_in_use() {
        let mut names = GlNames::default();
        names.mark_used(1);
        names.mark_used(0);
        assert_eq!(names.generate(), 2);
        assert_eq!(names.in_use().collect::<Vec<_>>(), vec![1, 2]);
    }
}

#[cfg(test)]
mod buffer_tests {
    use super::buffer_slice;

    #[test]
    fn offset_zero_is_the_start_of_the_buffer() {
        // It is the most common case: with a buffer bound, `glVertexPointer(…, 0)` is not a null
        // pointer — it is "from the start of the buffer". Reading the game's memory at address
        // zero was exactly what made a game with a VBO die on an invalid access.
        let content = [1u8, 2, 3, 4, 5, 6];
        assert_eq!(buffer_slice(&content, 0, 3), vec![1, 2, 3]);
    }

    #[test]
    fn the_offset_moves_within_the_buffer() {
        let content = [1u8, 2, 3, 4, 5, 6];
        assert_eq!(buffer_slice(&content, 4, 2), vec![5, 6]);
    }

    #[test]
    fn what_runs_past_the_end_comes_out_zeroed_instead_of_failing() {
        let content = [1u8, 2, 3, 4];
        assert_eq!(buffer_slice(&content, 3, 4), vec![0, 0, 0, 0]);
        assert_eq!(buffer_slice(&content, 9, 2), vec![0, 0]);
        // And a buffer that was never even filled answers the same, without a special case.
        assert_eq!(buffer_slice(&[], 0, 3), vec![0, 0, 0]);
    }

    #[test]
    fn a_large_offset_does_not_overflow_the_sum() {
        // `address + count` in a 32-bit `usize` would wrap around and the slice would pass the
        // bounds check. `saturating_add` is what prevents that — and the case exists: a corrupted
        // array pointer arrives here as a huge offset.
        assert_eq!(buffer_slice(&[1, 2, 3], u32::MAX, 8), vec![0u8; 8]);
    }
}
