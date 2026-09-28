//! O navegador de pastas.
//!
//! É um navegador nosso, e não o seletor do sistema, por uma razão de fundo: o
//! `ACTION_OPEN_DOCUMENT_TREE` devolve um `content://`, e o carregador do núcleo abre caminho
//! de arquivo. Um navegador sobre o `std::fs` devolve o que ele já sabe usar — e não custa uma
//! linha de JNI.
//!
//! Serve a duas escolhas: a pasta de ROMs ([`Onde::Seletor`]) e o banco `.sf2` da música MIDI
//! ([`Onde::SeletorDeBanco`]). As duas navegam pelas pastas do mesmo jeito.

use std::path::{Path, PathBuf};

use crate::{Emulador, Onde};

impl Emulador {
    pub(crate) fn seletor(&mut self, ctx: &egui::Context, atual: &Path) {
        // O que a pasta tem: as subpastas para navegar, e quantos jogos para decidir.
        let mut pastas: Vec<PathBuf> = Vec::new();
        let mut modulos = 0usize;
        let mut ilegivel = None;
        match std::fs::read_dir(atual) {
            Ok(entradas) => {
                for entrada in entradas.flatten() {
                    let caminho = entrada.path();
                    if caminho.is_dir() {
                        pastas.push(caminho);
                    } else if e_modulo(&caminho) {
                        modulos += 1;
                    }
                }
                pastas.sort();
            }
            Err(erro) => ilegivel = Some(erro.to_string()),
        }

        egui::TopBottomPanel::top("caminho").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(self.tr("settings.roms_folder"));
            ui.weak(atual.display().to_string());
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("acoes").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let largura = (ui.available_width() - 24.0) / 2.0;
                let cancelar = ui.add_sized([largura, 56.0], egui::Button::new("Cancelar"));
                if cancelar.clicked() {
                    self.onde = Onde::Ajustes;
                }
                // O mesmo dos ajustes: o egui só move um foco que já existe, e do nada quem o
                // concede é o `Tab`. Sem este primeiro foco o direcional não anda aqui.
                if ctx.memory(|m| m.focused()).is_none() {
                    cancelar.request_focus();
                }
                let rotulo = match modulos {
                    0 => "Usar esta pasta (vazia)".to_string(),
                    1 => "Usar esta pasta (1 jogo)".to_string(),
                    n => format!("Usar esta pasta ({n} jogos)"),
                };
                if ui
                    .add_sized([largura, 56.0], egui::Button::new(rotulo))
                    .clicked()
                {
                    self.settings.roms_dir = Some(atual.to_path_buf());
                    self.salva();
                    self.onde = Onde::Biblioteca;
                    self.recarrega();
                }
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(destino) = self.navega_pastas(ui, atual, &pastas, ilegivel.as_deref()) {
                self.onde = Onde::Seletor(destino);
            }
        });
    }

    /// O navegador do banco `.sf2`: as mesmas pastas, e os bancos de cada uma para tocar e
    /// escolher.
    pub(crate) fn seletor_de_banco(&mut self, ctx: &egui::Context, atual: &Path) {
        let mut pastas: Vec<PathBuf> = Vec::new();
        let mut bancos: Vec<PathBuf> = Vec::new();
        let mut ilegivel = None;
        match std::fs::read_dir(atual) {
            Ok(entradas) => {
                for entrada in entradas.flatten() {
                    let caminho = entrada.path();
                    if caminho.is_dir() {
                        pastas.push(caminho);
                    } else if e_banco(&caminho) {
                        bancos.push(caminho);
                    }
                }
                pastas.sort();
                bancos.sort();
            }
            Err(erro) => ilegivel = Some(erro.to_string()),
        }

        egui::TopBottomPanel::top("caminho").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(self.tr("audio.soundfont"));
            ui.weak(atual.display().to_string());
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("acoes").show(ctx, |ui| {
            ui.add_space(8.0);
            let cancelar =
                ui.add_sized([ui.available_width(), 56.0], egui::Button::new("Cancelar"));
            if cancelar.clicked() {
                self.onde = Onde::Ajustes;
            }
            if ctx.memory(|m| m.focused()).is_none() {
                cancelar.request_focus();
            }
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut escolhido = None;
            // Os bancos vêm antes das pastas: numa pasta com eles, é isso que se veio buscar.
            for banco in &bancos {
                let nome = banco
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if ui
                    .add_sized(
                        [ui.available_width(), 52.0],
                        egui::Button::new(format!("🎵  {nome}")),
                    )
                    .clicked()
                {
                    escolhido = Some(banco.clone());
                }
            }
            if let Some(banco) = escolhido {
                // Vale a partir do próximo jogo: o banco é aberto quando a máquina nasce.
                self.settings.audio.soundfont = Some(banco);
                self.salva();
                self.onde = Onde::Ajustes;
                return;
            }
            if let Some(destino) = self.navega_pastas(ui, atual, &pastas, ilegivel.as_deref()) {
                self.onde = Onde::SeletorDeBanco(destino);
            }
        });
    }

    /// Os atalhos, o "subir" e as subpastas de `atual`. Devolve a pasta tocada, se alguma foi.
    fn navega_pastas(
        &self,
        ui: &mut egui::Ui,
        atual: &Path,
        pastas: &[PathBuf],
        ilegivel: Option<&str>,
    ) -> Option<PathBuf> {
        {
            // Ir direto aos dois lugares que sempre existem poupa uma dúzia de toques — e a
            // pasta do aplicativo é a única que dispensa permissão.
            let mut destino = None;
            ui.horizontal(|ui| {
                if ui.button("Armazenamento").clicked() {
                    destino = Some(PathBuf::from("/sdcard"));
                }
                if ui.button("Pasta do aplicativo").clicked() {
                    destino = Some(self.minha_pasta.clone());
                }
            });
            ui.add_space(4.0);

            if let Some(erro) = ilegivel {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Não deu para ler: {erro}"),
                );
                ui.label("Em Ajustes › Aplicativos › Zeebx, conceda \"acesso a todos os arquivos\".");
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(acima) = atual.parent() {
                    if ui
                        .add_sized([ui.available_width(), 52.0], egui::Button::new("⬆  .."))
                        .clicked()
                    {
                        destino = Some(acima.to_path_buf());
                    }
                }
                for pasta in pastas {
                    let nome = pasta
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if ui
                        .add_sized(
                            [ui.available_width(), 52.0],
                            egui::Button::new(format!("📁  {nome}")),
                        )
                        .clicked()
                    {
                        destino = Some(pasta.clone());
                    }
                }
            });
            destino
        }
    }
}

/// Um banco de instrumentos `.sf2`.
fn e_banco(caminho: &Path) -> bool {
    caminho
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("sf2"))
}

/// Um `.mod` é o jogo; um `.zip` é o pacote que o carregador abre.
fn e_modulo(caminho: &Path) -> bool {
    caminho
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mod") || e.eq_ignore_ascii_case("zip"))
}
