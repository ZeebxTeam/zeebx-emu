//! As configurações para o QML, e o idioma da interface.
//!
//! **Uma chave, e não uma propriedade por opção.** São dezenas de opções, e cada uma seria uma
//! propriedade com getter, setter e sinal. O QML lê e grava pela chave — o caminho no
//! `settings.json`, como `graphics.smooth` —, e o número de `versao` muda a cada gravação: as
//! ligações que o leem se refazem. Cada gravação salva o arquivo e aplica o efeito na hora, como no
//! egui: o volume vai para o jogo aberto, a resolução interna refaz o destino na placa.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qrectf.h");
        type QRectF = cxx_qt_lib::QRectF;
    }

    extern "RustQt" {
        /// O idioma da interface. As ligações do QML leem a `versao` pelo `tr()` de cada arquivo:
        /// trocar o idioma a muda, e todo texto se refaz na hora, como no egui.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, versao)]
        type Idioma = super::IdiomaRust;

        /// Um texto do catálogo, pela chave.
        #[qinvokable]
        fn texto(self: &Idioma, chave: &QString) -> QString;

        /// O idioma mudou: todo texto se refaz.
        #[qinvokable]
        fn recarrega(self: Pin<&mut Idioma>);
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, versao)]
        #[qproperty(i32, porta_editada, cxx_name = "portaEditada")]
        type Configuracoes = super::ConfiguracoesRust;

        /// O valor de uma opção: `bool`, número, ou texto. Nas escolhas de lista, o índice.
        #[qinvokable]
        fn valor(self: &Configuracoes, chave: &QString) -> QVariant;

        /// Grava uma opção, salva o arquivo e aplica o efeito.
        #[qinvokable]
        fn define(self: Pin<&mut Configuracoes>, chave: &QString, valor: &QVariant);

        /// Os rótulos das escolhas de uma opção de lista, já traduzidos, na ordem dos índices.
        #[qinvokable]
        fn opcoes(self: &Configuracoes, chave: &QString) -> QStringList;

        #[qinvokable]
        #[cxx_name = "escolhePastaDeRoms"]
        fn escolhe_pasta_de_roms(self: Pin<&mut Configuracoes>);

        /// Aponta a Z-Wheel: o `.zip` ou o `.mod` com `pasta` falso, ou a pasta que a contenha.
        #[qinvokable]
        #[cxx_name = "escolheZWheel"]
        fn escolhe_z_wheel(self: Pin<&mut Configuracoes>, pasta: bool);

        /// Procura a Z-Wheel sem perguntar: na pasta de ROMs, ao lado dela, em `Downloads`.
        #[qinvokable]
        #[cxx_name = "detectaZWheel"]
        fn detecta_z_wheel(self: Pin<&mut Configuracoes>);

        /// O que dizer da Z-Wheel apontada: achada com capas, achada sem, ou não achada.
        #[qinvokable]
        #[cxx_name = "estadoDaZWheel"]
        fn estado_da_z_wheel(self: &Configuracoes) -> QString;

        #[qinvokable]
        #[cxx_name = "zWheelAchada"]
        fn z_wheel_achada(self: &Configuracoes) -> bool;

        /// A pasta de ROMs configurada não existe mais.
        #[qinvokable]
        #[cxx_name = "pastaDeRomsFalta"]
        fn pasta_de_roms_falta(self: &Configuracoes) -> bool;

        #[qinvokable]
        #[cxx_name = "procuraAtualizacao"]
        fn procura_atualizacao(self: Pin<&mut Configuracoes>);

        #[qinvokable]
        #[cxx_name = "procurandoAtualizacao"]
        fn procurando_atualizacao(self: &Configuracoes) -> bool;

        /// O que a procura por versão nova respondeu, em texto; vazio antes de procurar.
        #[qinvokable]
        #[cxx_name = "estadoDaAtualizacao"]
        fn estado_da_atualizacao(self: &Configuracoes) -> QString;

        /// A página da versão nova, se há uma.
        #[qinvokable]
        #[cxx_name = "paginaDaAtualizacao"]
        fn pagina_da_atualizacao(self: &Configuracoes) -> QString;

        #[qinvokable]
        #[cxx_name = "discordConectado"]
        fn discord_conectado(self: &Configuracoes) -> bool;

        /// "As configurações ficam em …", com o caminho do arquivo.
        #[qinvokable]
        #[cxx_name = "ondeFicam"]
        fn onde_ficam(self: &Configuracoes) -> QString;

        #[qinvokable]
        #[cxx_name = "versaoDoEmulador"]
        fn versao_do_emulador(self: &Configuracoes) -> QString;

        /// O endereço do repositório, ou o convite do servidor de conversa, para a aba Sobre.
        #[qinvokable]
        fn endereco(self: &Configuracoes, qual: &QString) -> QString;

        // ---- A aba de controles, da porta em edição ------------------------------------------

        /// Troca a porta em edição. Uma captura aberta é abandonada: deixá-la mapearia a próxima
        /// tecla no botão da porta anterior.
        #[qinvokable]
        #[cxx_name = "editaPorta"]
        fn edita_porta(self: Pin<&mut Configuracoes>, porta: i32);

        #[qinvokable]
        #[cxx_name = "portaLigada"]
        fn porta_ligada(self: &Configuracoes) -> bool;

        #[qinvokable]
        #[cxx_name = "ligaPorta"]
        fn liga_porta(self: Pin<&mut Configuracoes>, ligada: bool);

        /// O que o console vê na porta: 0 Z-Pad, 1 controle, 2 Boomerang, 3 teclado.
        #[qinvokable]
        fn aparelho(self: &Configuracoes) -> i32;

        #[qinvokable]
        #[cxx_name = "defineAparelho"]
        fn define_aparelho(self: Pin<&mut Configuracoes>, aparelho: i32);

        /// Os controles do host que a porta pode ler: "nenhum" primeiro, depois os do `gilrs` e os
        /// Wii Remotes.
        #[qinvokable]
        fn controles(self: &Configuracoes) -> QStringList;

        /// O índice do controle da porta em `controles`.
        #[qinvokable]
        #[cxx_name = "controleAtual"]
        fn controle_atual(self: &Configuracoes) -> i32;

        #[qinvokable]
        #[cxx_name = "escolheControle"]
        fn escolhe_controle(self: Pin<&mut Configuracoes>, indice: i32);

        #[qinvokable]
        #[cxx_name = "restauraMapeamento"]
        fn restaura_mapeamento(self: Pin<&mut Configuracoes>);

        /// Os botões do Zeebo que se mapeiam, pelo nome interno.
        #[qinvokable]
        fn botoes(self: &Configuracoes) -> QStringList;

        /// De onde um botão vem, em texto: "Z, South", ou "sem atribuição".
        #[qinvokable]
        #[cxx_name = "origensDoBotao"]
        fn origens_do_botao(self: &Configuracoes, botao: &QString) -> QString;

        /// O botão esperando uma tecla ou um botão, ou vazio.
        #[qinvokable]
        fn capturando(self: &Configuracoes) -> QString;

        /// Começa a esperar uma origem para o botão; de novo no mesmo, desiste.
        #[qinvokable]
        fn captura(self: Pin<&mut Configuracoes>, botao: &QString);

        #[qinvokable]
        #[cxx_name = "limpaBotao"]
        fn limpa_botao(self: Pin<&mut Configuracoes>, botao: &QString);

        /// Um `Qt::Key` na janela de configurações: acende o botão no desenho, e com uma captura
        /// aberta vira a origem dele. Esc desiste da captura.
        #[qinvokable]
        fn tecla(self: Pin<&mut Configuracoes>, codigo: i32, apertada: bool);

        /// A leitura de cada quadro da aba: o controle, a captura por botão de controle e a
        /// calibração em andamento.
        #[qinvokable]
        #[cxx_name = "leControle"]
        fn le_controle(self: Pin<&mut Configuracoes>);

        #[qinvokable]
        fn apertado(self: &Configuracoes, botao: &QString) -> bool;

        /// A posição de um eixo da porta, de -1 a 1, ao vivo.
        #[qinvokable]
        fn eixo(self: &Configuracoes, indice: i32) -> f64;

        /// Os eixos do console, pelo nome interno.
        #[qinvokable]
        fn eixos(self: &Configuracoes) -> QStringList;

        /// As origens que um eixo aceita: "nenhum" primeiro, depois os eixos do controle.
        #[qinvokable]
        #[cxx_name = "origensDeEixo"]
        fn origens_de_eixo(self: &Configuracoes) -> QStringList;

        #[qinvokable]
        #[cxx_name = "origemDoEixo"]
        fn origem_do_eixo(self: &Configuracoes, eixo: &QString) -> i32;

        #[qinvokable]
        #[cxx_name = "defineOrigemDoEixo"]
        fn define_origem_do_eixo(self: Pin<&mut Configuracoes>, eixo: &QString, indice: i32);

        #[qinvokable]
        #[cxx_name = "eixoInvertido"]
        fn eixo_invertido(self: &Configuracoes, eixo: &QString) -> bool;

        #[qinvokable]
        #[cxx_name = "inverteEixo"]
        fn inverte_eixo(self: Pin<&mut Configuracoes>, eixo: &QString, invertido: bool);

        /// Largura sobre altura do desenho do controle, ou 0 sem desenho.
        #[qinvokable]
        #[cxx_name = "proporcaoDaArte"]
        fn proporcao_da_arte(self: &Configuracoes) -> f64;

        #[qinvokable]
        fn partes(self: &Configuracoes) -> QStringList;

        /// Onde a silhueta de um botão fica no desenho, de 0 a 1.
        #[qinvokable]
        #[cxx_name = "limitesDaParte"]
        fn limites_da_parte(self: &Configuracoes, indice: i32) -> QRectF;

        /// O botão sob o ponto, pela silhueta, e não por uma caixa: um direcional em cruz e um
        /// botão redondo respondem só onde existe botão.
        #[qinvokable]
        #[cxx_name = "botaoEm"]
        fn botao_em(self: &Configuracoes, u: f64, v: f64) -> QString;

        /// O sensor de movimento da porta, em texto.
        #[qinvokable]
        fn sensor(self: &Configuracoes) -> QString;

        #[qinvokable]
        #[cxx_name = "sensorSemPermissao"]
        fn sensor_sem_permissao(self: &Configuracoes) -> bool;

        #[qinvokable]
        #[cxx_name = "comLeitura"]
        fn com_leitura(self: &Configuracoes) -> bool;

        /// A aceleração, o ângulo de volante e o que está apertado, numa linha.
        #[qinvokable]
        #[cxx_name = "linhaDoMovimento"]
        fn linha_do_movimento(self: &Configuracoes) -> QString;

        /// Quanto a imagem do Boomerang gira, em graus.
        #[qinvokable]
        fn giro(self: &Configuracoes) -> f64;

        #[qinvokable]
        fn calibrando(self: &Configuracoes) -> bool;

        #[qinvokable]
        #[cxx_name = "calibracaoRecusada"]
        fn calibracao_recusada(self: &Configuracoes) -> bool;

        #[qinvokable]
        fn calibra(self: Pin<&mut Configuracoes>);

        #[qinvokable]
        #[cxx_name = "restauraCalibracao"]
        fn restaura_calibracao(self: Pin<&mut Configuracoes>);

        /// Pede a senha do sistema e grava a regra do udev que libera os sensores.
        #[qinvokable]
        #[cxx_name = "liberaSensores"]
        fn libera_sensores(self: Pin<&mut Configuracoes>);

        /// 0 parada, 1 pedindo a senha, 2 feita, 3 falhou.
        #[qinvokable]
        #[cxx_name = "estadoDaLiberacao"]
        fn estado_da_liberacao(self: &Configuracoes) -> i32;

        /// O que dizer quando a liberação falhou, com o arquivo da regra.
        #[qinvokable]
        #[cxx_name = "falhaDaLiberacao"]
        fn falha_da_liberacao(self: &Configuracoes) -> QString;

        /// A regra do udev, para ser criada à mão quando o pedido falha.
        #[qinvokable]
        #[cxx_name = "regraDoUdev"]
        fn regra_do_udev(self: &Configuracoes) -> QString;

        /// A pasta de ROMs ou a Z-Wheel mudaram: a biblioteca refaz a lista, varrendo o disco de
        /// novo quando `varrer`.
        #[qsignal]
        #[cxx_name = "bibliotecaMudou"]
        fn biblioteca_mudou(self: Pin<&mut Configuracoes>, varrer: bool);

        #[qsignal]
        #[cxx_name = "idiomaMudou"]
        fn idioma_mudou(self: Pin<&mut Configuracoes>);

        /// O modo da janela principal mudou: vale na hora, como no egui. O da janela do jogo vale
        /// no próximo jogo.
        #[qsignal]
        #[cxx_name = "janelaPrincipalMudou"]
        fn janela_principal_mudou(self: Pin<&mut Configuracoes>);
    }
}

use std::collections::HashSet;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QRectF, QString, QStringList, QVariant};

use zeebx::eframe::egui::Key;
use zeebx::input::bindings::{Aparelho, AxisSource, CONFIGURABLE, CalibracaoDeMovimento, Source};
use zeebx::input::gamepads;
use zeebx::input::sensores;
use zeebx::input::wiimote::Wiimotes;
use zeebx::input::{AXIS_CURSO, AXIS_NAMES, Pad};
use zeebx::ui::entrada::SensorDaPorta;

use zeebx::library;
use zeebx::ui::atualizacao::{self, Resposta};
use zeebx::ui::settings::{
    self, ModoDaBiblioteca, ModoDaJanela, Proporcao, Scaling, Settings, rotulo_da_resolucao,
    rotulo_de_nivel,
};

use super::nucleo;

#[derive(Default)]
pub struct IdiomaRust {
    versao: i32,
}

impl qobject::Idioma {
    pub fn texto(&self, chave: &QString) -> QString {
        let chave = String::from(chave);
        nucleo::com(|nucleo| QString::from(nucleo.catalogo.get(&chave)))
    }

    pub fn recarrega(mut self: Pin<&mut Self>) {
        let versao = self.versao().wrapping_add(1);
        self.as_mut().set_versao(versao);
    }
}

#[derive(Default)]
pub struct ConfiguracoesRust {
    versao: i32,
    porta_editada: i32,
    /// O botão esperando uma tecla ou um botão de controle.
    capturando: Option<String>,
    /// As teclas apertadas na janela de configurações, para acender o desenho.
    teclas: HashSet<Key>,
    /// A calibração em andamento: a porta e as leituras paradas juntadas até agora.
    calibrando: Option<(usize, Vec<[f32; 3]>)>,
    /// A última calibração foi recusada por não estar de face para cima.
    calibracao_recusada: bool,
    /// O pedido de permissão para os sensores, que corre numa thread enquanto a janela de senha
    /// do sistema está aberta.
    liberacao: Arc<Mutex<Liberacao>>,
}

/// O pedido de permissão para os sensores.
#[derive(Debug, Clone, Default)]
enum Liberacao {
    #[default]
    Parada,
    /// A janela de senha do sistema está aberta.
    Pedindo,
    Feita,
    Falhou(String),
}

/// Quantas leituras paradas a calibração do movimento junta: meio segundo a 100 por segundo.
const AMOSTRAS_DA_CALIBRACAO: usize = 50;

/// Os aparelhos que a porta pode ser, na ordem dos índices do QML.
const APARELHOS: [Aparelho; 4] = [Aparelho::ZPad, Aparelho::Controle, Aparelho::Boomerang, Aparelho::Teclado];

/// Os níveis de antialias e de anisotrópico que a aba gráfica oferece, como no egui.
const ANTIALIAS: [u8; 4] = [1, 2, 4, 8];
const ANISOTROPICO: [u8; 5] = [1, 2, 4, 8, 16];

/// O que uma gravação pede além de salvar.
#[derive(Default)]
struct Efeito {
    biblioteca: Option<bool>,
    idioma: bool,
    janela_principal: bool,
}

/// As opções de liga e desliga, pela chave.
fn booleano<'a>(settings: &'a mut Settings, chave: &str) -> Option<&'a mut bool> {
    Some(match chave {
        "z_wheel.fim_de_vida" => &mut settings.z_wheel.fim_de_vida,
        "z_wheel.transicoes_sempre" => &mut settings.z_wheel.transicoes_sempre,
        "discord.ativo" => &mut settings.discord.ativo,
        "atualizacoes.ao_abrir" => &mut settings.atualizacoes.ao_abrir,
        "graphics.smooth" => &mut settings.graphics.smooth,
        "graphics.keep_aspect" => &mut settings.graphics.keep_aspect,
        "graphics.speed_limit" => &mut settings.graphics.speed_limit,
        "graphics.neblina" => &mut settings.graphics.neblina,
        "graphics.gpu_rasterizer" => &mut settings.graphics.gpu_rasterizer,
        "audio.enabled" => &mut settings.audio.enabled,
        "debug.overlay" => &mut settings.debug.overlay,
        "debug.speed" => &mut settings.debug.speed,
        "debug.clock" => &mut settings.debug.clock,
        "debug.memory" => &mut settings.debug.memory,
        "debug.timeline" => &mut settings.debug.timeline,
        "debug.log" => &mut settings.debug.log,
        "movimento.aviso_de_calibracao" => &mut settings.movimento.aviso_de_calibracao,
        _ => return None,
    })
}

fn indice<T: PartialEq>(opcoes: &[T], valor: &T) -> i32 {
    opcoes.iter().position(|opcao| opcao == valor).unwrap_or(0) as i32
}

fn escolha<T: Copy>(opcoes: &[T], indice: i32) -> Option<T> {
    usize::try_from(indice).ok().and_then(|i| opcoes.get(i).copied())
}

impl qobject::Configuracoes {
    pub fn valor(&self, chave: &QString) -> QVariant {
        let chave = String::from(chave);
        nucleo::com(|nucleo| {
            let caminho = |caminho: &Option<PathBuf>| {
                QVariant::from(&QString::from(
                    &caminho.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                ))
            };
            if let Some(valor) = booleano(&mut nucleo.settings, &chave) {
                return QVariant::from(&*valor);
            }
            let graficos = &nucleo.settings.graphics;
            let numero = match chave.as_str() {
                "roms_dir" => return caminho(&nucleo.settings.roms_dir),
                "z_wheel_path" => return caminho(&nucleo.settings.z_wheel_path),
                "language" => {
                    let atual = nucleo.catalogo.current();
                    nucleo
                        .catalogo
                        .languages()
                        .iter()
                        .position(|idioma| idioma.code == atual)
                        .unwrap_or(0) as i32
                }
                "biblioteca" => indice(
                    &[ModoDaBiblioteca::Grade, ModoDaBiblioteca::Slider],
                    &nucleo.settings.biblioteca,
                ),
                "graphics.janela" => indice(&ModoDaJanela::TODOS, &graficos.janela),
                "graphics.janela_do_jogo" => indice(&ModoDaJanela::TODOS, &graficos.janela_do_jogo),
                "graphics.scaling" => indice(&Scaling::ALL, &graficos.scaling),
                "graphics.proporcao" => indice(&Proporcao::TODAS, &graficos.proporcao),
                "graphics.resolucao_interna" => i32::from(graficos.resolucao_interna.clamp(1, 6)) - 1,
                "graphics.antialias" => indice(&ANTIALIAS, &graficos.antialias),
                "graphics.anisotropico" => indice(&ANISOTROPICO, &graficos.anisotropico),
                "audio.volume" => i32::from(nucleo.settings.audio.volume),
                _ => {
                    eprintln!("configuração desconhecida: {chave}");
                    return QVariant::default();
                }
            };
            QVariant::from(&numero)
        })
    }

    pub fn define(mut self: Pin<&mut Self>, chave: &QString, valor: &QVariant) {
        let chave = String::from(chave);
        let ligado = valor.value::<bool>().unwrap_or_default();
        let numero = valor.value::<i32>().unwrap_or_default();
        let efeito = nucleo::com(|nucleo| {
            let mut efeito = Efeito::default();
            if let Some(campo) = booleano(&mut nucleo.settings, &chave) {
                *campo = ligado;
            }
            let graficos = &mut nucleo.settings.graphics;
            match chave.as_str() {
                "language" => {
                    let codigo = usize::try_from(numero)
                        .ok()
                        .and_then(|i| nucleo.catalogo.languages().get(i))
                        .map(|idioma| idioma.code.clone());
                    if let Some(codigo) = codigo {
                        efeito.idioma = nucleo.muda_idioma(&codigo);
                        efeito.biblioteca = Some(false);
                    }
                }
                "biblioteca" => {
                    let modos = [ModoDaBiblioteca::Grade, ModoDaBiblioteca::Slider];
                    nucleo.settings.biblioteca = escolha(&modos, numero).unwrap_or_default();
                }
                "graphics.janela" => {
                    graficos.janela = escolha(&ModoDaJanela::TODOS, numero).unwrap_or_default();
                    efeito.janela_principal = true;
                }
                "graphics.janela_do_jogo" => {
                    graficos.janela_do_jogo = escolha(&ModoDaJanela::TODOS, numero).unwrap_or_default();
                }
                "graphics.scaling" => {
                    graficos.scaling = escolha(&Scaling::ALL, numero).unwrap_or_default();
                }
                // Vale na hora para o jogo aberto: o destino é refeito no próximo quadro.
                "graphics.proporcao" => {
                    graficos.proporcao = escolha(&Proporcao::TODAS, numero).unwrap_or_default();
                    let aspecto = graficos.proporcao.aspecto(16.0 / 9.0);
                    nucleo.na_sessao(|sessao| sessao.define_proporcao(aspecto));
                }
                "graphics.resolucao_interna" => {
                    graficos.resolucao_interna = (numero + 1).clamp(1, 6) as u8;
                    let fator = graficos.resolucao_interna as usize;
                    nucleo.na_sessao(|sessao| sessao.define_resolucao_interna(fator));
                }
                "graphics.antialias" | "graphics.anisotropico" => {
                    if chave == "graphics.antialias" {
                        graficos.antialias = escolha(&ANTIALIAS, numero).unwrap_or(1);
                    } else {
                        graficos.anisotropico = escolha(&ANISOTROPICO, numero).unwrap_or(1);
                    }
                    let (amostras, nivel) = (graficos.antialias as usize, graficos.anisotropico as usize);
                    nucleo.na_sessao(|sessao| sessao.define_melhorias(amostras, nivel));
                }
                // Vale na hora: o próximo desenho já sai com ou sem névoa.
                "graphics.neblina" => {
                    let permitida = graficos.neblina;
                    nucleo.na_sessao(|sessao| sessao.define_neblina(permitida));
                }
                // Mexer no volume com o jogo aberto tem que valer na hora, não só na próxima
                // abertura.
                "audio.enabled" | "audio.volume" => {
                    if chave == "audio.volume" {
                        nucleo.settings.audio.volume = numero.clamp(0, 100) as u8;
                    }
                    let (ligado, volume) = (nucleo.settings.audio.enabled, nucleo.settings.audio.volume);
                    nucleo.na_sessao(|sessao| {
                        sessao.set_audio(ligado, volume);
                    });
                }
                _ => {}
            }
            salva(&nucleo.settings);
            efeito
        });
        self.as_mut().aplica(efeito);
    }

    pub fn opcoes(&self, chave: &QString) -> QStringList {
        let chave = String::from(chave);
        let rotulos: Vec<String> = nucleo::com(|nucleo| {
            let catalogo = &nucleo.catalogo;
            let traduz = |chaves: &[&str]| chaves.iter().map(|c| catalogo.get(c).to_string()).collect();
            let desligado = catalogo.get("common.off");
            match chave.as_str() {
                "language" => catalogo.languages().iter().map(|l| l.name.clone()).collect(),
                "biblioteca" => {
                    traduz(&["settings.library_view.grid", "settings.library_view.slider"])
                }
                "graphics.janela" | "graphics.janela_do_jogo" => {
                    ModoDaJanela::TODOS.iter().map(|m| catalogo.get(m.chave()).to_string()).collect()
                }
                "graphics.scaling" => {
                    Scaling::ALL.iter().map(|e| catalogo.get(e.key()).to_string()).collect()
                }
                "graphics.proporcao" => {
                    Proporcao::TODAS.iter().map(|p| catalogo.get(p.chave()).to_string()).collect()
                }
                "graphics.resolucao_interna" => (1..=6u8).map(rotulo_da_resolucao).collect(),
                "graphics.antialias" => {
                    ANTIALIAS.iter().map(|&n| rotulo_de_nivel(n, "MSAA", desligado)).collect()
                }
                "graphics.anisotropico" => {
                    ANISOTROPICO.iter().map(|&n| rotulo_de_nivel(n, "AF", desligado)).collect()
                }
                _ => Vec::new(),
            }
        });
        lista_de_textos(rotulos)
    }

    pub fn escolhe_pasta_de_roms(mut self: Pin<&mut Self>) {
        let Some(pasta) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        nucleo::com(|nucleo| {
            nucleo.settings.roms_dir = Some(pasta);
            salva(&nucleo.settings);
        });
        self.as_mut().aplica(Efeito {
            biblioteca: Some(true),
            ..Efeito::default()
        });
    }

    pub fn escolhe_z_wheel(mut self: Pin<&mut Self>, pasta: bool) {
        let escolhida = match pasta {
            true => rfd::FileDialog::new().pick_folder(),
            false => rfd::FileDialog::new().add_filter("Z-Wheel", &["zip", "mod"]).pick_file(),
        };
        if let Some(caminho) = escolhida {
            self.as_mut().aponta_z_wheel(caminho);
        }
    }

    pub fn detecta_z_wheel(mut self: Pin<&mut Self>) {
        let achada = nucleo::com(|nucleo| {
            library::detecta_z_wheel(nucleo.settings.roms_dir.as_deref(), &nucleo.jogos)
        });
        if let Some(caminho) = achada {
            self.as_mut().aponta_z_wheel(caminho);
        }
    }

    pub fn estado_da_z_wheel(&self) -> QString {
        nucleo::com(|nucleo| {
            let catalogo = &nucleo.catalogo;
            QString::from(&match (&nucleo.z_wheel, &nucleo.acervo) {
                (Some(_), Some(acervo)) => catalogo
                    .format("settings.z_wheel.found", &[("count", &acervo.len().to_string())]),
                (Some(_), None) => catalogo.get("settings.z_wheel.found_no_art").to_string(),
                (None, _) => catalogo.get("settings.z_wheel.not_found").to_string(),
            })
        })
    }

    pub fn z_wheel_achada(&self) -> bool {
        nucleo::com(|nucleo| nucleo.z_wheel.is_some())
    }

    pub fn pasta_de_roms_falta(&self) -> bool {
        nucleo::com(|nucleo| nucleo.settings.roms_dir.as_ref().is_some_and(|p| !p.is_dir()))
    }

    pub fn procura_atualizacao(self: Pin<&mut Self>) {
        nucleo::com(|nucleo| nucleo.procura_atualizacao());
    }

    pub fn procurando_atualizacao(&self) -> bool {
        nucleo::com(|nucleo| nucleo.procurando_atualizacao())
    }

    pub fn estado_da_atualizacao(&self) -> QString {
        nucleo::com(|nucleo| {
            let catalogo = &nucleo.catalogo;
            let texto = match (&nucleo.atualizacao, nucleo.procurando_atualizacao()) {
                (_, true) => catalogo.get("settings.updates.checking").to_string(),
                (Some(Resposta::Nova(lancamento)), _) => {
                    catalogo.format("settings.updates.new", &[("version", &lancamento.versao)])
                }
                (Some(Resposta::EmDia), _) => catalogo.get("settings.updates.up_to_date").to_string(),
                (Some(Resposta::Falhou(motivo)), _) => {
                    catalogo.format("settings.updates.failed", &[("reason", motivo)])
                }
                (None, false) => String::new(),
            };
            QString::from(&texto)
        })
    }

    pub fn pagina_da_atualizacao(&self) -> QString {
        nucleo::com(|nucleo| match &nucleo.atualizacao {
            Some(Resposta::Nova(lancamento)) => QString::from(&lancamento.pagina),
            _ => QString::default(),
        })
    }

    pub fn discord_conectado(&self) -> bool {
        nucleo::com(|nucleo| nucleo.presenca.conectado())
    }

    pub fn onde_ficam(&self) -> QString {
        nucleo::com(|nucleo| {
            let caminho = settings::settings_path().display().to_string();
            QString::from(&nucleo.catalogo.format("settings.saved_at", &[("path", &caminho)]))
        })
    }

    pub fn versao_do_emulador(&self) -> QString {
        QString::from(atualizacao::VERSAO_ATUAL)
    }

    pub fn endereco(&self, qual: &QString) -> QString {
        QString::from(match String::from(qual).as_str() {
            "discord" => zeebx::ui::DISCORD,
            _ => zeebx::ui::REPOSITORIO,
        })
    }

    fn aponta_z_wheel(mut self: Pin<&mut Self>, caminho: PathBuf) {
        nucleo::com(|nucleo| {
            nucleo.settings.z_wheel_path = Some(caminho);
            salva(&nucleo.settings);
            nucleo.atualiza_z_wheel();
        });
        self.as_mut().aplica(Efeito {
            biblioteca: Some(false),
            ..Efeito::default()
        });
    }

    /// Avisa o QML do que mudou. Fora do `nucleo::com`: os sinais chamam o QML na hora.
    fn aplica(mut self: Pin<&mut Self>, efeito: Efeito) {
        let versao = self.versao().wrapping_add(1);
        self.as_mut().set_versao(versao);
        if efeito.idioma {
            self.as_mut().idioma_mudou();
        }
        if let Some(varrer) = efeito.biblioteca {
            self.as_mut().biblioteca_mudou(varrer);
        }
        if efeito.janela_principal {
            self.as_mut().janela_principal_mudou();
        }
    }
}

fn salva(settings: &Settings) {
    if let Err(erro) = settings.save() {
        eprintln!("não deu para guardar as configurações: {erro}");
    }
}

/// A aba de controles, sobre a porta em edição.
impl qobject::Configuracoes {
    fn porta(&self) -> usize {
        usize::try_from(*self.porta_editada()).unwrap_or(0).min(zeebx::input::PORTAS - 1)
    }

    /// O controle da porta agora, com o teclado desta janela.
    fn pad(&self, nucleo: &nucleo::Nucleo) -> Pad {
        nucleo.entrada.pad(&nucleo.settings.controls, self.porta(), &self.rust().teclas)
    }

    /// Os controles da porta mudaram: salva, e com um jogo aberto vale na hora.
    fn controles_mudaram(mut self: Pin<&mut Self>) {
        nucleo::com(|nucleo| {
            salva(&nucleo.settings);
            nucleo.portas_mudaram();
        });
        let versao = self.versao().wrapping_add(1);
        self.as_mut().set_versao(versao);
    }

    pub fn edita_porta(mut self: Pin<&mut Self>, porta: i32) {
        self.as_mut().rust_mut().capturando = None;
        self.as_mut().set_porta_editada(porta);
        let versao = self.versao().wrapping_add(1);
        self.as_mut().set_versao(versao);
    }

    pub fn porta_ligada(&self) -> bool {
        let porta = self.porta();
        nucleo::com(|nucleo| nucleo.settings.controls.player(porta).is_some_and(|j| j.ligada))
    }

    pub fn liga_porta(mut self: Pin<&mut Self>, ligada: bool) {
        let porta = self.porta();
        nucleo::com(|nucleo| nucleo.settings.controls.player_mut(porta).ligada = ligada);
        self.as_mut().controles_mudaram();
    }

    pub fn aparelho(&self) -> i32 {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            let aparelho = nucleo.settings.controls.player(porta).map(|j| j.aparelho);
            aparelho.map_or(1, |aparelho| indice(&APARELHOS, &aparelho))
        })
    }

    pub fn define_aparelho(mut self: Pin<&mut Self>, aparelho: i32) {
        let porta = self.porta();
        let Some(aparelho) = escolha(&APARELHOS, aparelho) else {
            return;
        };
        nucleo::com(|nucleo| nucleo.settings.controls.player_mut(porta).aparelho = aparelho);
        self.as_mut().controles_mudaram();
    }

    /// Os controles do host que a porta pode escolher: os do `gilrs`, os Wii Remotes e, por
    /// último, o configurado na porta mesmo que não esteja ligado agora. Sem ele, um controle
    /// desligado aparecia como "somente teclado", e quem abrisse a lista via a configuração
    /// errada; o egui mostra o nome guardado do mesmo jeito.
    fn nomes_dos_controles(nucleo: &nucleo::Nucleo, porta: usize) -> Vec<String> {
        let mut nomes = nucleo.entrada.gamepads.names();
        nomes.extend((0..nucleo.entrada.wiimotes.quantos()).map(Wiimotes::nome));
        let configurado = nucleo.settings.controls.player(porta).and_then(|j| j.device.clone());
        if let Some(configurado) = configurado.filter(|nome| !nomes.contains(nome)) {
            nomes.push(configurado);
        }
        nomes
    }

    pub fn controles(&self) -> QStringList {
        let porta = self.porta();
        let nomes = nucleo::com(|nucleo| {
            let mut nomes = vec![nucleo.catalogo.get("controls.device.none").to_string()];
            nomes.extend(Self::nomes_dos_controles(nucleo, porta));
            nomes
        });
        lista_de_textos(nomes)
    }

    pub fn controle_atual(&self) -> i32 {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            let Some(device) = nucleo.settings.controls.player(porta).and_then(|j| j.device.clone()) else {
                return 0;
            };
            let nomes = Self::nomes_dos_controles(nucleo, porta);
            nomes.iter().position(|nome| *nome == device).map_or(0, |i| i as i32 + 1)
        })
    }

    pub fn escolhe_controle(mut self: Pin<&mut Self>, indice: i32) {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            let nomes = Self::nomes_dos_controles(nucleo, porta);
            let device = usize::try_from(indice - 1).ok().and_then(|i| nomes.get(i).cloned());
            nucleo.settings.controls.player_mut(porta).troca_controle(device);
        });
        self.as_mut().controles_mudaram();
    }

    pub fn restaura_mapeamento(mut self: Pin<&mut Self>) {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            let jogador = nucleo.settings.controls.player_mut(porta);
            *jogador = zeebx::input::bindings::Player::padrao_do_controle(jogador.device.clone());
        });
        self.as_mut().controles_mudaram();
    }

    pub fn botoes(&self) -> QStringList {
        lista_de_textos(CONFIGURABLE.iter().map(|b| b.to_string()).collect())
    }

    pub fn origens_do_botao(&self, botao: &QString) -> QString {
        let (porta, botao) = (self.porta(), String::from(botao));
        nucleo::com(|nucleo| {
            let origens = nucleo
                .settings
                .controls
                .player(porta)
                .map(|jogador| jogador.sources(&botao).to_vec())
                .unwrap_or_default();
            QString::from(&match origens.is_empty() {
                true => nucleo.catalogo.get("controls.unbound").to_string(),
                false => origens.iter().map(Source::label).collect::<Vec<_>>().join(", "),
            })
        })
    }

    pub fn capturando(&self) -> QString {
        QString::from(self.rust().capturando.as_deref().unwrap_or_default())
    }

    pub fn captura(mut self: Pin<&mut Self>, botao: &QString) {
        let botao = String::from(botao);
        let mut rust = self.as_mut().rust_mut();
        // Clicar de novo no mesmo botão desiste da captura.
        rust.capturando = match rust.capturando.as_deref() == Some(botao.as_str()) {
            true => None,
            false => Some(botao),
        };
    }

    pub fn limpa_botao(mut self: Pin<&mut Self>, botao: &QString) {
        let (porta, botao) = (self.porta(), String::from(botao));
        nucleo::com(|nucleo| nucleo.settings.controls.player_mut(porta).clear(&botao));
        self.as_mut().controles_mudaram();
    }

    pub fn tecla(mut self: Pin<&mut Self>, codigo: i32, apertada: bool) {
        let Some(tecla) = super::ponte::tecla_do_qt(codigo) else {
            return;
        };
        let porta = self.porta();
        let mut rust = self.as_mut().rust_mut();
        match apertada {
            true => rust.teclas.insert(tecla),
            false => rust.teclas.remove(&tecla),
        };
        if !apertada {
            return;
        }
        let Some(botao) = rust.capturando.take() else {
            return;
        };
        if tecla == Key::Escape {
            return;
        }
        drop(rust);
        // O teclado primeiro: é o que está debaixo da mão de quem está configurando.
        nucleo::com(|nucleo| {
            nucleo
                .settings
                .controls
                .player_mut(porta)
                .bind(&botao, Source::key(tecla.name()));
        });
        self.as_mut().controles_mudaram();
    }

    pub fn le_controle(mut self: Pin<&mut Self>) {
        let porta = self.porta();
        let capturando = self.rust().capturando.clone();
        let mut calibrando = self.as_mut().rust_mut().calibrando.take();
        let mut recusada = None;
        let mudou = nucleo::com(|nucleo| {
            // **O gilrs só atualiza o estado quando a fila de eventos é drenada.** Sem isto o
            // desenho ficava apagado por mais que se apertasse.
            nucleo.entrada.gamepads.poll();
            let controles = &nucleo.settings.controls;
            let mut mudou = false;
            if let Some(botao) = &capturando {
                let device = controles.player(porta).and_then(|j| j.device.clone());
                // O Wii Remote escolhido responde pelos botões dele; os outros, pelo gilrs.
                let origem = match nucleo.entrada.wiimote_da_porta(controles, porta) {
                    Some(wiimote) if device.is_some() => wiimote.primeira_fonte(),
                    _ => nucleo.entrada.gamepads.first_active(device.as_deref(), porta),
                };
                if let Some(origem) = origem {
                    nucleo.settings.controls.player_mut(porta).bind(botao, origem);
                    mudou = true;
                }
            }
            // A calibração em andamento junta leituras paradas e fecha na média.
            let bruto = nucleo.entrada.movimento_bruto_da_porta(&nucleo.settings.controls, porta);
            if let (Some((de, amostras)), Some(bruto)) = (&mut calibrando, bruto)
                && *de == porta
            {
                amostras.push(bruto);
                if amostras.len() >= AMOSTRAS_DA_CALIBRACAO {
                    let n = amostras.len() as f32;
                    let media: [f32; 3] =
                        std::array::from_fn(|i| amostras.iter().map(|a| a[i]).sum::<f32>() / n);
                    match CalibracaoDeMovimento::de_repouso(media) {
                        Some(calibracao) => {
                            nucleo.settings.controls.player_mut(porta).calibracao_movimento =
                                calibracao;
                            recusada = Some(false);
                            mudou = true;
                        }
                        None => recusada = Some(true),
                    }
                    calibrando = None;
                }
            }
            mudou
        });
        let mut rust = self.as_mut().rust_mut();
        rust.calibrando = calibrando;
        if let Some(recusada) = recusada {
            rust.calibracao_recusada = recusada;
        }
        if mudou {
            rust.capturando = None;
            drop(rust);
            self.as_mut().controles_mudaram();
        }
    }

    pub fn apertado(&self, botao: &QString) -> bool {
        let botao = String::from(botao);
        nucleo::com(|nucleo| {
            let pad = self.pad(nucleo);
            Pad::button_by_name(&botao).is_some_and(|i| pad.is_down(i))
        })
    }

    pub fn eixo(&self, indice: i32) -> f64 {
        nucleo::com(|nucleo| {
            let pad = self.pad(nucleo);
            let valor = usize::try_from(indice).ok().and_then(|i| pad.axes.get(i).copied());
            f64::from(valor.unwrap_or(0)) / f64::from(AXIS_CURSO)
        })
    }

    pub fn eixos(&self) -> QStringList {
        lista_de_textos(AXIS_NAMES.iter().map(|e| e.to_string()).collect())
    }

    pub fn origens_de_eixo(&self) -> QStringList {
        let nenhum = nucleo::com(|nucleo| nucleo.catalogo.get("controls.axis.none").to_string());
        lista_de_textos(std::iter::once(nenhum).chain(gamepads::axis_names().map(String::from)).collect())
    }

    pub fn origem_do_eixo(&self, eixo: &QString) -> i32 {
        let (porta, eixo) = (self.porta(), String::from(eixo));
        nucleo::com(|nucleo| {
            let origem = nucleo.settings.controls.player(porta).and_then(|j| j.axes.get(&eixo).cloned());
            origem
                .and_then(|origem| gamepads::axis_names().position(|nome| nome == origem.name))
                .map_or(0, |i| i as i32 + 1)
        })
    }

    pub fn define_origem_do_eixo(mut self: Pin<&mut Self>, eixo: &QString, indice: i32) {
        let (porta, eixo) = (self.porta(), String::from(eixo));
        nucleo::com(|nucleo| {
            let jogador = nucleo.settings.controls.player_mut(porta);
            let nome = usize::try_from(indice - 1).ok().and_then(|i| gamepads::axis_names().nth(i));
            match nome {
                Some(nome) => {
                    let invertido = jogador.axes.get(&eixo).is_some_and(|s| s.invert);
                    jogador.axes.insert(
                        eixo,
                        AxisSource {
                            name: nome.to_string(),
                            invert: invertido,
                        },
                    );
                }
                None => {
                    jogador.axes.remove(&eixo);
                }
            }
        });
        self.as_mut().controles_mudaram();
    }

    pub fn eixo_invertido(&self, eixo: &QString) -> bool {
        let (porta, eixo) = (self.porta(), String::from(eixo));
        nucleo::com(|nucleo| {
            let jogador = nucleo.settings.controls.player(porta);
            jogador.and_then(|j| j.axes.get(&eixo)).is_some_and(|s| s.invert)
        })
    }

    pub fn inverte_eixo(mut self: Pin<&mut Self>, eixo: &QString, invertido: bool) {
        let (porta, eixo) = (self.porta(), String::from(eixo));
        nucleo::com(|nucleo| {
            if let Some(origem) = nucleo.settings.controls.player_mut(porta).axes.get_mut(&eixo) {
                origem.invert = invertido;
            }
        });
        self.as_mut().controles_mudaram();
    }

    pub fn proporcao_da_arte(&self) -> f64 {
        nucleo::com(|nucleo| {
            nucleo.arte.as_ref().map_or(0.0, |arte| arte.width as f64 / arte.height as f64)
        })
    }

    pub fn partes(&self) -> QStringList {
        let nomes = nucleo::com(|nucleo| {
            let partes = nucleo.arte.as_ref().map(|arte| arte.parts()).unwrap_or_default();
            partes.iter().map(|parte| parte.button.clone()).collect()
        });
        lista_de_textos(nomes)
    }

    pub fn limites_da_parte(&self, indice: i32) -> QRectF {
        nucleo::com(|nucleo| {
            let Some(arte) = &nucleo.arte else {
                return QRectF::default();
            };
            let Some(parte) = usize::try_from(indice).ok().and_then(|i| arte.parts().get(i)) else {
                return QRectF::default();
            };
            let [u0, v0, u1, v1] = parte.bounds(arte).map(f64::from);
            QRectF::new(u0, v0, u1 - u0, v1 - v0)
        })
    }

    pub fn botao_em(&self, u: f64, v: f64) -> QString {
        nucleo::com(|nucleo| {
            let botao = nucleo.arte.as_ref().and_then(|arte| arte.hit(u as f32, v as f32));
            QString::from(botao.unwrap_or_default())
        })
    }

    pub fn sensor(&self) -> QString {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            let sensor = nucleo.entrada.sensor_da_porta(&nucleo.settings.controls, porta);
            QString::from(&sensor.descreve(&nucleo.catalogo))
        })
    }

    pub fn sensor_sem_permissao(&self) -> bool {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            let sensor = nucleo.entrada.sensor_da_porta(&nucleo.settings.controls, porta);
            matches!(sensor, SensorDaPorta::Controle(s) if s.sem_permissao)
        })
    }

    pub fn com_leitura(&self) -> bool {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            nucleo.entrada.movimento_bruto_da_porta(&nucleo.settings.controls, porta).is_some()
        })
    }

    pub fn linha_do_movimento(&self) -> QString {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            let [x, y, z] = nucleo.entrada.movimento_da_porta(&nucleo.settings.controls, porta);
            let pad = self.pad(nucleo);
            let nomes: Vec<&str> = ["up", "down", "left", "right", "b1", "b2", "back"]
                .into_iter()
                .filter(|nome| Pad::button_by_name(nome).is_some_and(|i| pad.is_down(i)))
                .collect();
            let volante = x.atan2(y).to_degrees();
            QString::from(&format!(
                "x {x:+.2}  y {y:+.2}  z {z:+.2} g   ⟲ {volante:+.0}°  {}",
                nomes.join(" ")
            ))
        })
    }

    pub fn giro(&self) -> f64 {
        let porta = self.porta();
        nucleo::com(|nucleo| {
            // O ângulo de volante, o mesmo que o Crash Nitro Kart calcula: a gravidade no plano da
            // face. Deitado, ela sai desse plano e o ângulo vira ruído, então a imagem só gira
            // quando a gravidade está de fato ali.
            let [x, y, _] = nucleo.entrada.movimento_da_porta(&nucleo.settings.controls, porta);
            let no_plano = (x * x + y * y).sqrt();
            let giro = x.atan2(y) * ((no_plano - 0.3) / 0.4).clamp(0.0, 1.0);
            f64::from(giro).to_degrees()
        })
    }

    pub fn calibrando(&self) -> bool {
        self.rust().calibrando.is_some()
    }

    pub fn calibracao_recusada(&self) -> bool {
        self.rust().calibracao_recusada
    }

    pub fn calibra(mut self: Pin<&mut Self>) {
        let porta = self.porta();
        self.as_mut().rust_mut().calibrando = Some((porta, Vec::new()));
    }

    pub fn restaura_calibracao(mut self: Pin<&mut Self>) {
        let porta = self.porta();
        self.as_mut().rust_mut().calibrando = None;
        nucleo::com(|nucleo| {
            nucleo.settings.controls.player_mut(porta).calibracao_movimento = Default::default();
        });
        self.as_mut().controles_mudaram();
    }

    pub fn libera_sensores(mut self: Pin<&mut Self>) {
        let liberacao = Arc::clone(&self.as_mut().rust_mut().liberacao);
        if let Ok(mut estado) = liberacao.lock() {
            *estado = Liberacao::Pedindo;
        }
        let _ = std::thread::Builder::new()
            .name("libera-sensores".into())
            .spawn(move || {
                let resultado = match sensores::libera_sensores() {
                    Ok(()) => Liberacao::Feita,
                    Err(erro) => Liberacao::Falhou(erro),
                };
                if let Ok(mut estado) = liberacao.lock() {
                    *estado = resultado;
                }
            });
    }

    pub fn estado_da_liberacao(&self) -> i32 {
        match self.rust().liberacao.lock().map(|e| e.clone()) {
            Ok(Liberacao::Pedindo) => 1,
            Ok(Liberacao::Feita) => 2,
            Ok(Liberacao::Falhou(_)) => 3,
            _ => 0,
        }
    }

    pub fn falha_da_liberacao(&self) -> QString {
        let erro = match self.rust().liberacao.lock().map(|e| e.clone()) {
            Ok(Liberacao::Falhou(erro)) => erro,
            _ => return QString::default(),
        };
        nucleo::com(|nucleo| {
            QString::from(
                &nucleo
                    .catalogo
                    .get("controls.boomerang.unlock_failed")
                    .replace("{erro}", &erro)
                    .replace("{arquivo}", sensores::ARQUIVO_DA_REGRA),
            )
        })
    }

    pub fn regra_do_udev(&self) -> QString {
        QString::from(sensores::REGRA_DO_UDEV)
    }
}

fn lista_de_textos(textos: Vec<String>) -> QStringList {
    let mut lista = QList::<QString>::default();
    for texto in textos {
        lista.append(QString::from(&texto));
    }
    QStringList::from(&lista)
}
