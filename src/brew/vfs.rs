//! Sistema de arquivos visto pelo jogo.
//!
//! O BREW dá a cada módulo um diretório próprio, e é para lá que os caminhos do jogo apontam.
//! Aqui traduzimos os caminhos do guest para o diretório do módulo no host, recusando qualquer
//! coisa que tente sair dele.
//!
//! Formas de caminho que aparecem nos jogos e no SDK:
//!
//! ```text
//! arquivo.dat          relativo ao diretório do módulo
//! fs:/~/arquivo.dat    o "~" é o diretório do próprio módulo
//! fs:/~/../id1/x       sobe para a raiz de módulos — o Quake guarda os dados dele assim
//! fs:/~0x01234567/x    diretório de outro módulo, pelo ClassID
//! fs:/shared/x         área compartilhada entre aplicativos
//! fs:/zeeboiddata/x    **o sistema de arquivos do aparelho**, comum a todos
//! ```
//!
//! # O `fs:/` é do aparelho, não do jogo
//!
//! Esta distinção parece detalhe e não é. No console há **um** sistema de arquivos: o Zeeboids
//! grava os bonecos em `fs:/zeeboiddata/zeeboid.db` e o Zeebo F.C. abre esse mesmo caminho para
//! importá-los. Enquanto `fs:/` caía dentro da pasta do módulo, cada jogo via uma pasta só sua
//! e vazia — e o F.C. concluía, corretamente do ponto de vista dele, que o Zeeboids não estava
//! instalado.
//!
//! Então: caminho relativo e `fs:/~/` vão para a pasta do módulo; `fs:/` vai para uma raiz
//! comum, que é o que o console tem.

use std::path::{Path, PathBuf};

/// Prefixos que o BREW usa e que removemos antes de resolver.
const PREFIXES: [&str; 4] = ["fs:/~/", "fs:/~", "fs:/", "~/"];

/// Tira o `<classe>/` que pode vir logo depois do `~`.
///
/// O BREW escreve o diretório de um módulo como `fs:/~<ClassID>/`, e a Z-Wheel usa essa forma
/// para os próprios recursos: `fs:/~0x01070798/tectoyli.brf`. Sem tirar o número, o caminho
/// virava um **subdiretório** com nome `0x01070798` dentro do diretório do módulo — que não
/// existe. O arquivo estava no pacote e mesmo assim não abria.
///
/// Foi o que segurou o formulário de instruções do z-pad da Z-Wheel: ele carrega um recurso do
/// `tectoyli.brf` por esse caminho, não achava, e devolvia `EUNABLETOLOAD` — o erro 6 que
/// aparecia milhares de vezes no relatório.
///
/// **Aqui só existe um módulo por vez**, então qualquer ClassID leva ao diretório dele. No
/// console um módulo pode alcançar o diretório de outro por este caminho; quando isso importar,
/// é aqui que se resolve o número para o diretório certo.
fn sem_classe(resto: &str) -> String {
    let Some((primeiro, cauda)) = resto.split_once('/') else {
        return resto.to_string();
    };
    let numero = primeiro
        .strip_prefix("0x")
        .or_else(|| primeiro.strip_prefix("0X"))
        .map(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| !primeiro.is_empty() && primeiro.chars().all(|c| c.is_ascii_digit()));
    match numero {
        true => cauda.to_string(),
        false => resto.to_string(),
    }
}
/// Nome da raiz comum, que faz o papel do sistema de arquivos do aparelho.
const DEVICE_DIR: &str = "aparelho";

/// Para que serve uma abertura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenIntent {
    /// Ler um arquivo que precisa existir.
    Read,
    /// Criar um nome novo, sem sobrescrever um existente de caixa diferente.
    Create,
    /// Ler e escrever o mesmo arquivo, preservando o conteúdo atual.
    ReadWrite,
    /// Acrescentar ao fim do arquivo.
    Append,
}

/// Onde abrir, e a cópia que precisa preceder a escrita.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    /// Caminho do host a abrir.
    pub path: PathBuf,
    /// Arquivo do pacote a copiar para `path` antes de escrever, no copy-on-write.
    pub copy_from: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Vfs {
    /// Diretório do módulo — onde caem os caminhos relativos e o `~`.
    ///
    /// É o **conteúdo**: pacote extraído ou instalação ao lado do `.mod`. Tratado como
    /// somente-leitura a partir do momento em que existe um overlay.
    root: PathBuf,
    /// Overlay gravável do título, quando o frontend fornece um.
    ///
    /// Sem ele o emulador grava dentro do próprio conteúdo, que é o comportamento histórico do
    /// desktop. Com ele, o pacote nunca é alterado e o save sobrevive a reextrair a ROM.
    save: Option<PathBuf>,
    /// A raiz comum, que faz o papel do sistema de arquivos do aparelho.
    device: PathBuf,
    /// Até onde o `..` pode subir. No console é a raiz de módulos: o Quake mantém os dados
    /// dele em `mod/id1/`, ao lado do diretório do próprio módulo, e chega lá por
    /// `fs:/~/../id1/`.
    boundary: PathBuf,
}

impl Vfs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let boundary = root.parent().map(Path::to_path_buf).unwrap_or(root.clone());
        // A raiz comum fica ao lado das pastas de módulo, dentro da instalação do jogo. Um nível
        // acima disso ficaria fora do que cada jogo pode alcançar pelo `..`.
        let device = boundary.join(DEVICE_DIR);
        Self {
            root,
            boundary,
            device,
            save: None,
        }
    }

    /// Liga o overlay gravável do título. Ver [`Vfs::open_target`].
    pub fn set_save_root(&mut self, save: impl Into<PathBuf>) {
        self.save = Some(save.into());
    }

    /// Aponta a raiz comum para outro lugar.
    ///
    /// Existe para que **todos os jogos** compartilhem o mesmo sistema de arquivos, e não uma
    /// cópia por instalação: é assim que o Zeebo F.C. encontra os bonecos que o Zeeboids gravou.
    pub fn set_device_root(&mut self, device: impl Into<PathBuf>) {
        self.device = device.into();
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Arquivo de estado criado pelo emulador para um aplicativo do console.
    ///
    /// Não passa pelo caminho do módulo: é estado do aparelho, portanto sobrevive a uma nova
    /// extração do ZIP e nunca altera o pacote original.
    pub fn profile_file(&self, app: &str, name: &str) -> PathBuf {
        self.device.join(app).join(name)
    }

    /// Traduz um caminho do guest para um caminho do host.
    ///
    /// Devolve `None` para qualquer caminho que escape da raiz — `..`, caminho absoluto do
    /// host ou raiz do Windows. O jogo não tem por que sair do diretório dele, e um `.mod` de
    /// origem desconhecida não deveria conseguir ler o resto da máquina.
    pub fn resolve(&self, guest_path: &str) -> Option<PathBuf> {
        let caminho = self.resolve_inner(guest_path, false).map(match_case);
        let caminho = self.ou_na_raiz_de_modulos(guest_path, caminho, false);
        self.overlay_existente(&caminho).or(caminho)
    }

    /// O mesmo caminho dentro do overlay, quando ele já existe lá.
    ///
    /// Um arquivo gravado pelo jogo esconde o do pacote: é o que faz o save relido ser o save, e
    /// não o recurso original.
    fn overlay_existente(&self, conteudo: &Option<PathBuf>) -> Option<PathBuf> {
        let save = self.save.as_deref()?;
        let relativo = conteudo.as_ref()?.strip_prefix(&self.root).ok()?;
        let candidato = match_case(save.join(relativo));
        candidato.exists().then_some(candidato)
    }

    /// Onde abrir um caminho, e de onde copiar antes de escrever.
    ///
    /// Ver [`OpenIntent`]. Sem overlay configurado o resultado é o comportamento histórico: o
    /// jogo grava ao lado do `.mod`.
    pub fn open_target(&self, guest_path: &str, intent: OpenIntent) -> Option<OpenTarget> {
        // Caminho do aparelho (`fs:/` sem `~`) é persistente e compartilhado: não passa pelo
        // overlay do título.
        let normalizado = guest_path.replace('\\', "/");
        let do_aparelho = normalizado.starts_with("fs:/") && !normalizado.starts_with("fs:/~");
        if do_aparelho || self.save.is_none() {
            return self.legacy_target(guest_path, intent);
        }
        let base = self.resolve_inner(guest_path, false)?;
        // Só o que vive dentro do conteúdo do título tem overlay. `fs:/~/../id1/x` fica de fora.
        let Ok(relativo) = base.strip_prefix(&self.root) else {
            return self.legacy_target(guest_path, intent);
        };
        let save = self.save.as_deref()?;
        let destino_pai = match_case_parent(save.join(relativo));
        let existente_no_overlay = match_case(save.join(relativo));
        let no_overlay = existente_no_overlay
            .exists()
            .then_some(existente_no_overlay);
        let conteudo = match_case(base.clone());
        let no_conteudo = conteudo.exists().then_some(conteudo);
        match intent {
            OpenIntent::Read => Some(OpenTarget {
                path: no_overlay.or(no_conteudo)?,
                copy_from: None,
            }),
            OpenIntent::Create => Some(OpenTarget {
                path: no_overlay.unwrap_or(destino_pai),
                copy_from: None,
            }),
            // Abrir para escrita um recurso do pacote copia-o antes: o pacote fica intacto e as
            // escritas seguintes já encontram o arquivo no overlay.
            OpenIntent::ReadWrite | OpenIntent::Append => Some(OpenTarget {
                path: no_overlay.clone().unwrap_or(destino_pai),
                copy_from: match no_overlay {
                    Some(_) => None,
                    None => no_conteudo,
                },
            }),
        }
    }

    /// Se o caminho já está dentro do overlay gravável.
    pub fn is_overlay_path(&self, path: &Path) -> bool {
        self.save
            .as_deref()
            .is_some_and(|save| path.starts_with(save))
    }

    /// O caminho equivalente no overlay, exista ele ou não.
    ///
    /// É o par de [`Vfs::overlay_dir`] para quem vai **remover** ou **renomear**: um caminho do
    /// pacote não pode ser apagado nem movido, porque o pacote é imutável.
    pub fn overlay_path(&self, guest_path: &str) -> Option<PathBuf> {
        let save = self.save.as_deref()?;
        let base = self.resolve_inner(guest_path, false)?;
        let relativo = base.strip_prefix(&self.root).ok()?;
        Some(match_case_parent(save.join(relativo)))
    }

    /// O diretório equivalente no overlay, quando ele existe.
    ///
    /// Serve à listagem: o jogo precisa enxergar os arquivos que ele mesmo gravou, e não só os
    /// que vieram no pacote.
    pub fn overlay_dir(&self, guest_path: &str) -> Option<PathBuf> {
        let save = self.save.as_deref()?;
        let base = self.resolve_inner(guest_path, true)?;
        let relativo = base.strip_prefix(&self.root).ok()?;
        let caminho = match_case(save.join(relativo));
        caminho.is_dir().then_some(caminho)
    }

    /// Comportamento histórico, sem overlay: grava dentro do próprio conteúdo.
    fn legacy_target(&self, guest_path: &str, intent: OpenIntent) -> Option<OpenTarget> {
        let path = match intent {
            OpenIntent::Read | OpenIntent::ReadWrite | OpenIntent::Append => {
                self.resolve(guest_path)?
            }
            OpenIntent::Create => self.resolve_new(guest_path)?,
        };
        Some(OpenTarget {
            path,
            copy_from: None,
        })
    }

    /// **Sobre o `preloaded.cfg`.**
    ///
    /// Ele lista os jogos que vêm de fábrica no aparelho, e nem o pacote da Z-Wheel nem o
    /// sistema de arquivos do dump o têm. O [`Machine`](crate::machine::Machine) o materializa
    /// no perfil persistente do aparelho quando a Z-Wheel o consulta, em vez de alterar o
    /// pacote original. A lista pode começar vazia: jogos locais pertencem ao banco da
    /// biblioteca, não a uma NAND inventada.
    /// Como [`Vfs::resolve`], mas sem a busca sem caixa.
    ///
    /// É para quem vai **criar** um nome, não abrir um existente: renomear para `save.dat`
    /// quando existe um `SAVE.DAT` tem de criar o primeiro, não sobrescrever o segundo.
    pub fn resolve_new(&self, guest_path: &str) -> Option<PathBuf> {
        // O nome novo fica exatamente como o guest escreveu, mas os diretórios que já existem
        // continuam seguindo a semântica sem caixa do console. Sem isto, `UDATA/save.dat`
        // criaria uma segunda pasta ao lado de `udata/` num host Linux.
        self.resolve_inner(guest_path, false).map(match_case_parent)
    }

    /// Como [`Vfs::resolve`], mas aceita o próprio diretório do módulo.
    ///
    /// Listar `fs:/~/` é legítimo — é o diretório do jogo —, ainda que abri-lo como arquivo não
    /// seja. A diferença está só nisso.
    pub fn resolve_dir(&self, guest_path: &str) -> Option<PathBuf> {
        let caminho = self.resolve_inner(guest_path, true).map(match_case);
        self.ou_na_raiz_de_modulos(guest_path, caminho, true)
    }

    /// `fs:/mod/<pasta>/…` que não existe no aparelho, procurado na raiz de módulos.
    ///
    /// No console, `fs:/mod/` é onde os módulos estão instalados, e `fs:/mod/<pasta>/` é a pasta
    /// de um deles. Os ports feitos por fãs usam essa forma para os próprios dados: o OpenTyrian
    /// abre `fs:/mod/opentyrian_zeebo/data/tyrian1.lvl`, não achava, e fechava no primeiro
    /// milissegundo. Aqui o `fs:/` é do aparelho, e a Z-Wheel guarda o que é dela em
    /// `fs:/mod/274755/` dessa raiz; por isso o que existe lá continua ganhando, e o que só
    /// existe na instalação do jogo vem dela. Um arquivo novo continua sendo criado no aparelho.
    fn ou_na_raiz_de_modulos(
        &self,
        guest_path: &str,
        caminho: Option<PathBuf>,
        allow_root: bool,
    ) -> Option<PathBuf> {
        let normalizado = guest_path.replace('\\', "/");
        let Some(resto) = normalizado.strip_prefix("fs:/mod/") else {
            return caminho;
        };
        if caminho.as_ref().is_some_and(|c| c.exists()) {
            return caminho;
        }
        // A pasta do módulo fica um nível abaixo da raiz de módulos, e o `..` dela não sobe mais.
        self.resolve_inner(&format!("../{resto}"), allow_root)
            .map(match_case)
            .filter(|instalado| instalado.exists())
            .or(caminho)
    }

    fn resolve_inner(&self, guest_path: &str, allow_root: bool) -> Option<PathBuf> {
        let mut path = guest_path.replace('\\', "/");
        for prefix in PREFIXES {
            if let Some(rest) = path.strip_prefix(prefix) {
                path = sem_classe(rest);
                break;
            }
        }
        // O que sobrou depois de tirar o prefixo diz onde a busca começa. Um caminho que veio
        // com `fs:/` e **não** era `fs:/~` é do aparelho, não do módulo.
        let do_aparelho = guest_path.starts_with("fs:/") && !guest_path.starts_with("fs:/~");
        let mut resolved = match do_aparelho {
            true => self.device.clone(),
            false => self.root.clone(),
        };
        let base = resolved.clone();
        // Uma barra vazia — `/` no começo ou `//` no meio — ancora o caminho na pasta do
        // módulo, e dali o `..` não sobe. Os Zeebo Extreme gravam o recorde das pistas em
        // `./udata/trackinfo.txt` e o releem pelo empacotador, que monta `"%s/%s"` com a raiz
        // `./`: `.//../udata/trackinfo.txt`. Subindo, a releitura caía em `mod/udata`, a lista
        // de pistas ficava sem vetor e a largada do Bóia Cross lia o ponteiro nulo.
        let mut ancorado = false;
        for part in path.split('/') {
            match part {
                "" => ancorado = !do_aparelho,
                "." => continue,
                ".." if ancorado && resolved == self.root => continue,
                // Subir é permitido até a raiz de módulos e nem um passo além.
                ".." => {
                    // Do aparelho não se sobe: ele já é a raiz. Da pasta do módulo sobe-se até
                    // a raiz de módulos, que é como o Quake alcança os dados dele.
                    let limite = match do_aparelho {
                        true => &base,
                        false => &self.boundary,
                    };
                    if resolved == *limite || !resolved.pop() {
                        return None;
                    }
                }
                _ if part.contains(':') => return None,
                _ => resolved.push(part),
            }
        }
        if !allow_root && (resolved == base || resolved == self.boundary) {
            return None;
        }
        Some(resolved)
    }
}

/// Resolve só os diretórios de um caminho novo, preservando o último componente para criação.
fn match_case_parent(path: PathBuf) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path;
    };
    match_case(parent.to_path_buf()).join(name)
}

/// O mesmo caminho, com a caixa que os arquivos têm de verdade no disco.
///
/// O sistema de arquivos do console não distingue maiúsculas de minúsculas, e os jogos contam
/// com isso: os dez ports de arcade do Zeebo pedem `font.fnz` e trazem `font.FNZ` no pacote.
/// Nenhum deles passava do `EVT_APP_START` — a fonte não abria e o ponteiro nulo vinha logo
/// depois. No Windows e no macOS isso funcionava por acaso; no Linux, não.
///
/// Um caminho que não existe volta como veio: é o caso de um arquivo que o jogo está criando.
fn match_case(path: PathBuf) -> PathBuf {
    if path.exists() {
        return path;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path;
    };
    // O diretório também pode estar com outra caixa, então ele é resolvido primeiro.
    let parent = match_case(parent.to_path_buf());
    let exact = parent.join(name);
    if exact.exists() {
        return exact;
    }
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return exact;
    };
    let alvo = name.to_string_lossy().to_lowercase();
    // Havendo mais de um candidato — dois arquivos que só diferem na caixa —, fica o primeiro
    // em ordem alfabética, para a escolha não depender da ordem em que o disco os devolve.
    entries
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|nome| nome.to_string_lossy().to_lowercase() == alvo)
        .min()
        .map_or(exact, |nome| parent.join(nome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_diretorio_do_modulo_pode_ser_listado_mas_nao_aberto() {
        // Listar `fs:/~/` é legítimo — é a pasta do jogo. Abri-la como arquivo não é, e a
        // diferença entre as duas respostas é justamente essa.
        let vfs = Vfs::new("/roms/jogo");
        assert_eq!(vfs.resolve("fs:/~/"), None);
        assert_eq!(
            vfs.resolve_dir("fs:/~/"),
            Some(std::path::PathBuf::from("/roms/jogo"))
        );
        // Sair da raiz continua proibido nos dois.
        assert_eq!(vfs.resolve_dir("fs:/~/../../etc"), None);
    }

    fn vfs() -> Vfs {
        Vfs::new("/jogos/bjt")
    }

    #[test]
    fn caminho_relativo_cai_na_raiz_do_modulo() {
        assert_eq!(
            vfs().resolve("dados.dat"),
            Some(PathBuf::from("/jogos/bjt/dados.dat"))
        );
    }

    #[test]
    fn o_til_e_do_modulo_e_o_resto_e_do_aparelho() {
        let vfs = vfs();
        // Com `~`, e sem prefixo nenhum, é a pasta do jogo.
        let do_jogo = Some(PathBuf::from("/jogos/bjt/dados.dat"));
        assert_eq!(vfs.resolve("fs:/~/dados.dat"), do_jogo);
        assert_eq!(vfs.resolve("~/dados.dat"), do_jogo);

        // Sem o `~`, é o sistema de arquivos do aparelho — comum a todos os jogos. É o que
        // permite ao Zeebo F.C. abrir `fs:/zeeboiddata/zeeboid.db`, que o Zeeboids gravou.
        assert_eq!(
            vfs.resolve("fs:/dados.dat"),
            Some(PathBuf::from("/jogos/aparelho/dados.dat"))
        );
        assert_eq!(
            vfs.resolve("fs:/zeeboiddata/zeeboid.db"),
            Some(PathBuf::from("/jogos/aparelho/zeeboiddata/zeeboid.db"))
        );
    }

    #[test]
    fn aceita_barra_invertida_como_separador() {
        assert_eq!(
            vfs().resolve("imagens\\fundo.bmp"),
            Some(PathBuf::from("/jogos/bjt/imagens/fundo.bmp"))
        );
    }

    #[test]
    fn a_area_compartilhada_e_do_aparelho() {
        assert_eq!(
            vfs().resolve("fs:/shared/save.dat"),
            Some(PathBuf::from("/jogos/aparelho/shared/save.dat"))
        );
    }

    #[test]
    fn do_aparelho_nao_se_sobe() {
        // A raiz do aparelho é raiz: `..` a partir dela sairia para a máquina do usuário.
        let vfs = vfs();
        assert_eq!(vfs.resolve("fs:/../x"), None);
        assert_eq!(vfs.resolve("fs:/zeeboiddata/../../x"), None);
    }

    #[test]
    fn sobe_ate_a_raiz_de_modulos_mas_nao_alem() {
        let vfs = Vfs::new("/jogos/mod/274802");
        // O Quake guarda os dados dele num diretório irmão.
        assert_eq!(
            vfs.resolve("fs:/~/../id1/splash_title.png"),
            Some(PathBuf::from("/jogos/mod/id1/splash_title.png"))
        );
        // Mais um nível já sairia da árvore de módulos.
        assert_eq!(vfs.resolve("fs:/~/../../outro/x"), None);
        assert_eq!(vfs.resolve("../../../etc/passwd"), None);
    }

    #[test]
    fn recusa_qualquer_tentativa_de_sair_da_raiz() {
        let vfs = vfs();
        // Um `..` chega à raiz de módulos, que é legítimo; dois já sairiam dela.
        assert_eq!(
            vfs.resolve("dados/../../fora.txt"),
            Some(PathBuf::from("/jogos/fora.txt"))
        );
        assert_eq!(vfs.resolve("../../etc/passwd"), None);
        assert_eq!(vfs.resolve("fs:/~/../../etc/passwd"), None);
        assert_eq!(vfs.resolve("C:/Windows/system32"), None);
    }

    #[test]
    fn depois_de_uma_barra_vazia_o_ponto_ponto_nao_sai_do_modulo() {
        let vfs = Vfs::new("/jogos/mod/278285");
        // O que o Bóia Cross grava e o caminho por onde ele relê.
        assert_eq!(
            vfs.resolve("./udata/trackinfo.txt"),
            vfs.resolve(".//../udata/trackinfo.txt")
        );
        assert_eq!(
            vfs.resolve("/../udata/trackinfo.txt"),
            Some(PathBuf::from("/jogos/mod/278285/udata/trackinfo.txt"))
        );
        // Sem a barra vazia, continua subindo até a raiz de módulos, como o NFS precisa.
        assert_eq!(
            vfs.resolve("../nfsresources/x.bar"),
            Some(PathBuf::from("/jogos/mod/nfsresources/x.bar"))
        );
    }

    #[test]
    fn caminho_vazio_nao_resolve_para_a_propria_raiz() {
        let vfs = vfs();
        assert_eq!(vfs.resolve(""), None);
        assert_eq!(vfs.resolve("fs:/~"), None);
    }
}

#[cfg(test)]
mod tests_no_disco {
    use super::*;

    /// O sistema de arquivos distingue maiúsculas de minúsculas?
    ///
    /// **O APFS do macOS e o NTFS do Windows, por padrão, não distinguem**: `data` e `DATA` são a
    /// mesma pasta, `font.fnz` e `font.FNZ` são o mesmo arquivo. Nesses sistemas o caminho pedido
    /// já existe, a resposta do VFS é ele mesmo, e não há duas grafias para comparar — foi assim
    /// que dois testes desta seção passaram a falhar **só no macOS** no primeiro run de CI que
    /// chegou até eles. Onde o sistema funde as grafias, o que a busca sem caixa tem de garantir
    /// vem de graça do próprio sistema de arquivos.
    fn distingue_caixa(onde: &Path) -> bool {
        let alta = onde.join("zeebx-CAIXA");
        let baixa = onde.join("zeebx-caixa");
        std::fs::write(&alta, b"x").unwrap();
        let distingue = !baixa.exists();
        let _ = std::fs::remove_file(&alta);
        distingue
    }

    /// Os dez ports de arcade pedem `font.fnz` e trazem `font.FNZ`. No console dá na mesma.
    #[test]
    fn acha_o_arquivo_com_outra_caixa() {
        let raiz = std::env::temp_dir().join(format!("zeebx-vfs-{}", std::process::id()));
        let modulo = raiz.join("mod/279233");
        std::fs::create_dir_all(&modulo).unwrap();
        std::fs::write(modulo.join("font.FNZ"), b"fonte").unwrap();

        let vfs = Vfs::new(&modulo);
        let distingue = distingue_caixa(&modulo);
        if distingue {
            assert_eq!(vfs.resolve("font.fnz"), Some(modulo.join("font.FNZ")));
        } else {
            // Sistema que funde as duas grafias: a resposta é o nome pedido, e o arquivo é o
            // mesmo. A prova que interessa aqui é que a resposta abre o conteúdo do pacote.
            let achado = vfs.resolve("font.fnz").expect("respondeu pelo arquivo");
            assert_eq!(std::fs::read(&achado).unwrap(), b"fonte");
        }
        // O que não existe volta como veio numa busca existente.
        assert_eq!(vfs.resolve("save.dat"), Some(modulo.join("save.dat")));
        // Uma criação preserva o nome do arquivo, mas acha diretórios existentes sem caixa.
        std::fs::create_dir_all(modulo.join("udata")).unwrap();
        if distingue {
            assert_eq!(
                vfs.resolve_new("UDATA/save.dat"),
                Some(modulo.join("udata/save.dat"))
            );
        } else {
            assert!(vfs.resolve_new("UDATA/save.dat").is_some());
        }

        std::fs::remove_dir_all(&raiz).unwrap();
    }

    /// O overlay do título guarda o que o jogo grava, sem tocar no conteúdo do pacote.
    #[test]
    fn overlay_grava_fora_do_conteudo_e_esconde_o_recurso_do_pacote() {
        let raiz = std::env::temp_dir().join(format!("zeebx-vfs-overlay-{}", std::process::id()));
        let modulo = raiz.join("jogo/mod/1");
        let save = raiz.join("saves/abc");
        std::fs::create_dir_all(&modulo).unwrap();
        std::fs::create_dir_all(&save).unwrap();
        std::fs::write(modulo.join("config.ini"), b"do pacote").unwrap();

        let mut vfs = Vfs::new(&modulo);
        vfs.set_save_root(&save);

        // Leitura: o pacote é encontrado enquanto não há nada no overlay.
        assert_eq!(vfs.resolve("config.ini"), Some(modulo.join("config.ini")));

        // Escrita de recurso existente: copia antes e depois o overlay vence.
        let alvo = vfs
            .open_target("config.ini", OpenIntent::ReadWrite)
            .unwrap();
        assert_eq!(alvo.path, save.join("config.ini"));
        assert_eq!(alvo.copy_from, Some(modulo.join("config.ini")));
        std::fs::write(&alvo.path, b"do save").unwrap();
        assert_eq!(vfs.resolve("config.ini"), Some(save.join("config.ini")));
        assert_eq!(
            std::fs::read(modulo.join("config.ini")).unwrap(),
            b"do pacote"
        );

        // Arquivo novo: nasce no overlay.
        let novo = vfs
            .open_target("udata/recorde.dat", OpenIntent::Create)
            .unwrap();
        assert_eq!(novo.path, save.join("udata/recorde.dat"));
        assert_eq!(novo.copy_from, None);

        // O aparelho continua fora do overlay do título.
        let mut vfs = vfs;
        let aparelho = raiz.join("aparelho");
        vfs.set_device_root(&aparelho);
        let dispositivo = vfs
            .open_target("fs:/zeeboiddata/zeeboid.db", OpenIntent::Create)
            .unwrap();
        assert_eq!(dispositivo.path, aparelho.join("zeeboiddata/zeeboid.db"));

        std::fs::remove_dir_all(&raiz).unwrap();
    }

    /// Com overlay, o que o jogo gravou pode ser apagado e renomeado; o pacote não.
    #[test]
    fn overlay_protege_o_pacote_de_remocao_e_renomeio() {
        let raiz = std::env::temp_dir().join(format!("zeebx-vfs-rm-{}", std::process::id()));
        let modulo = raiz.join("jogo/mod/1");
        let save = raiz.join("saves/abc");
        std::fs::create_dir_all(&modulo).unwrap();
        std::fs::create_dir_all(&save).unwrap();
        std::fs::write(modulo.join("recurso.pak"), b"do pacote").unwrap();

        let mut vfs = Vfs::new(&modulo);
        vfs.set_save_root(&save);

        // O alvo de remoção de um recurso do pacote é o caminho do overlay, não o do pacote.
        assert_eq!(
            vfs.overlay_path("recurso.pak"),
            Some(save.join("recurso.pak"))
        );
        assert!(!vfs.is_overlay_path(&modulo.join("recurso.pak")));

        // Depois de copiado para o overlay, o alvo passa a ser dele.
        std::fs::write(save.join("recurso.pak"), b"do save").unwrap();
        assert!(vfs.is_overlay_path(&vfs.resolve("recurso.pak").unwrap()));

        std::fs::remove_dir_all(&raiz).unwrap();
    }

    /// O OpenTyrian abre `fs:/mod/opentyrian_zeebo/data/tyrian1.lvl`: é a pasta dele no console.
    #[test]
    fn fs_mod_acha_a_pasta_do_modulo_quando_o_aparelho_nao_tem() {
        let raiz = std::env::temp_dir().join(format!("zeebx-vfs-mod-{}", std::process::id()));
        let modulo = raiz.join("jogo/tyrian");
        let aparelho = raiz.join("aparelho");
        std::fs::create_dir_all(modulo.join("data")).unwrap();
        std::fs::create_dir_all(aparelho.join("mod/tyrian")).unwrap();
        std::fs::write(modulo.join("data/tyrian1.lvl"), b"fase").unwrap();
        std::fs::write(modulo.join("tyrian.cfg"), b"pacote").unwrap();
        std::fs::write(aparelho.join("mod/tyrian/tyrian.cfg"), b"gravado").unwrap();

        let mut vfs = Vfs::new(&modulo);
        vfs.set_device_root(&aparelho);
        if distingue_caixa(&raiz) {
            assert_eq!(
                vfs.resolve("fs:/mod/tyrian/DATA/tyrian1.lvl"),
                Some(modulo.join("data/tyrian1.lvl"))
            );
        } else {
            let achado = vfs
                .resolve("fs:/mod/tyrian/DATA/tyrian1.lvl")
                .expect("respondeu pela pasta");
            assert_eq!(std::fs::read(&achado).unwrap(), b"fase");
        }
        // O que o aparelho tem continua vindo dele.
        assert_eq!(
            vfs.resolve("fs:/mod/tyrian/tyrian.cfg"),
            Some(aparelho.join("mod/tyrian/tyrian.cfg"))
        );
        // Um arquivo novo é criado no aparelho, como antes.
        assert_eq!(
            vfs.resolve("fs:/mod/tyrian/novo.sav"),
            Some(aparelho.join("mod/tyrian/novo.sav"))
        );
        assert_eq!(
            vfs.resolve_dir("fs:/mod/tyrian/data"),
            Some(modulo.join("data"))
        );
        // E dali não se sobe para fora da instalação.
        assert_eq!(vfs.resolve("fs:/mod/../../segredo"), None);

        std::fs::remove_dir_all(&raiz).unwrap();
    }
}
