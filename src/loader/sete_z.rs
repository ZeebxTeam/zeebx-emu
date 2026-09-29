//! Jogos guardados em `.7z`.
//!
//! O acervo do Zeebo circula nos dois formatos, e um `.7z` que o emulador recusa é um jogo que
//! **"não carrega" sem explicação** — o mesmo sintoma de arquivo corrompido, sem pista nenhuma
//! para quem tentou abrir. Este módulo faz o mínimo que o emulador precisa de um `.7z`: listar os
//! nomes, ler um arquivo e extrair tudo para o cache.
//!
//! Um arquivo compactado é conteúdo **não confiável**, venha ele em que formato vier, então os
//! limites são os mesmos do zip: teto de entradas, teto por arquivo e teto do total descompactado.
//! O que muda é só quem descompacta.
//!
//! O formato não é decidido pela extensão, e sim pela **assinatura** `7z¼¯'`: um `.7z` renomeado
//! para `.zip` continua sendo um `.7z`, e é assim que ele aparece em acervo remexido à mão.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::archive::ArchiveLimits;

/// A assinatura de um `.7z`: `7z` e os quatro bytes `BC AF 27 1C`.
pub const MAGIA: [u8; 6] = [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c];

/// O atributo do Windows que marca um ponto de nova análise — o que um `.7z` de symlink traz.
const ATRIBUTO_LINK: u32 = 0x400;

/// O arquivo começa com a assinatura do `.7z`?
pub fn eh_sete_z(caminho: &Path) -> bool {
    let Ok(mut arquivo) = std::fs::File::open(caminho) else {
        return false;
    };
    let mut cabecalho = [0u8; MAGIA.len()];
    arquivo.read_exact(&mut cabecalho).is_ok() && cabecalho == MAGIA
}

/// Converte o nome de uma entrada em caminho relativo seguro.
///
/// Recusa o que um arquivo malicioso usaria para escrever fora da pasta de destino: componente
/// `..`, caminho absoluto, raiz de unidade do Windows (`C:`) e nome vazio. O separador é aceito
/// nas duas formas porque um `.7z` feito no Windows traz `\`.
fn caminho_seguro(nome: &str) -> Option<PathBuf> {
    let mut saida = PathBuf::new();
    for parte in nome.split(['/', '\\']) {
        if parte.is_empty() || parte == "." {
            continue;
        }
        if parte == ".." {
            return None;
        }
        // `C:` e afins: no Windows isso é absoluto; no Linux passaria como nome de pasta comum, e
        // é melhor recusar nos dois do que aceitar diferente em cada sistema.
        if parte.contains(':') {
            return None;
        }
        saida.push(parte);
    }
    (!saida.as_os_str().is_empty()).then_some(saida)
}

/// Os nomes de dentro do `.7z`, com o que é pasta.
///
/// Sai do **cabeçalho**, sem descompactar dado nenhum: escolher qual `.mod` abrir custa uma
/// leitura de metadado, e não a extração inteira. É o que permite a um `.7z` ser tratado como o
/// zip é tratado hoje — a diferença fica no descompactador, não no resto do emulador.
pub fn listar(caminho: &Path) -> Option<Vec<(String, bool)>> {
    let arquivo = sevenz_rust2::Archive::open(caminho).ok()?;
    if arquivo.files.len() > super::archive::MAX_ENTRIES {
        return None;
    }
    Some(
        arquivo
            .files
            .iter()
            .map(|entrada| (entrada.name.clone(), entrada.is_directory))
            .collect(),
    )
}

/// Abre o `.7z` para leitura.
///
/// No `wasm32` o `open` do crate não existe: foi escondido no pressuposto de que o alvo não tem
/// arquivo. O Emscripten tem, então a abertura do `File` fica aqui e o `new` faz o resto — é a
/// mesma função, sem o `cfg`.
fn abre_leitor(caminho: &Path) -> std::io::Result<sevenz_rust2::ArchiveReader<std::fs::File>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        sevenz_rust2::ArchiveReader::open(caminho, sevenz_rust2::Password::empty())
            .map_err(std::io::Error::other)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let arquivo = std::fs::File::open(caminho)?;
        sevenz_rust2::ArchiveReader::new(arquivo, sevenz_rust2::Password::empty())
            .map_err(std::io::Error::other)
    }
}

/// Lê um arquivo de dentro do `.7z`, com teto de bytes.
///
/// Um bloco sólido obriga a descompactar o que vem antes, então o custo pode passar do tamanho do
/// arquivo pedido — o teto continua valendo para o que **guardamos**, e a parada é imediata quando
/// a entrada chega.
pub fn ler(caminho: &Path, alvo: &str, teto: u64) -> Option<Vec<u8>> {
    let mut leitor = abre_leitor(caminho).ok()?;
    let mut achado: Option<Vec<u8>> = None;
    let mut falhou = false;
    let _ = leitor.for_each_entries(|entrada, fluxo| {
        if entrada.name != alvo {
            return Ok(true);
        }
        let mut dados = Vec::new();
        match fluxo.take(teto + 1).read_to_end(&mut dados) {
            Ok(_) if dados.len() as u64 <= teto => achado = Some(dados),
            _ => falhou = true,
        }
        Ok(false)
    });
    if falhou { None } else { achado }
}

/// Extrai o `.7z` inteiro para `destino`, com os limites do zip.
pub(crate) fn extrair(caminho: &Path, destino: &Path, limites: ArchiveLimits) -> std::io::Result<()> {
    let mut leitor = abre_leitor(caminho)?;
    let mut entradas = 0usize;
    let mut total = 0u64;
    let mut falha: Option<std::io::Error> = None;
    let resultado = leitor.for_each_entries(|entrada, fluxo| {
        if let Err(erro) = extrai_uma(entrada, fluxo, destino, &mut entradas, &mut total, limites) {
            falha = Some(erro);
            // Para na primeira recusa: continuar extraindo um arquivo que já foi recusado só
            // gasta tempo para chegar ao mesmo erro.
            return Ok(false);
        }
        Ok(true)
    });
    if let Some(erro) = falha {
        return Err(erro);
    }
    resultado.map_err(std::io::Error::other)
}

/// Escreve uma entrada, aplicando os tetos e recusando caminho inseguro.
fn extrai_uma(
    entrada: &sevenz_rust2::ArchiveEntry,
    fluxo: &mut dyn Read,
    destino: &Path,
    entradas: &mut usize,
    total: &mut u64,
    limites: ArchiveLimits,
) -> std::io::Result<()> {
    use super::archive::{MAX_FILE_BYTES, MAX_TOTAL_BYTES};

    *entradas += 1;
    if *entradas > limites.entries {
        return Err(std::io::Error::other("o 7z tem entradas demais"));
    }
    let Some(relativo) = caminho_seguro(&entrada.name) else {
        return Err(std::io::Error::other("o 7z contém caminho inseguro"));
    };
    if entrada.windows_attributes & ATRIBUTO_LINK != 0 {
        return Err(std::io::Error::other("o 7z contém link simbólico"));
    }
    let saida = destino.join(&relativo);
    if entrada.is_directory {
        std::fs::create_dir_all(&saida)?;
        return Ok(());
    }
    if let Some(pasta) = saida.parent() {
        std::fs::create_dir_all(pasta)?;
    }
    // O teto por arquivo vem do zip; o teto do total é recalculado a cada entrada para que um
    // cabeçalho mentiroso não escreva além dele durante a descompactação.
    let restante = MAX_TOTAL_BYTES
        .checked_sub(*total)
        .ok_or_else(|| std::io::Error::other("o 7z descompacta bytes demais"))?;
    let permitido = MAX_FILE_BYTES.min(restante);
    let mut arquivo = std::fs::File::create(&saida)?;
    let escrito = std::io::copy(&mut fluxo.take(permitido + 1), &mut arquivo)?;
    if escrito > permitido {
        return Err(std::io::Error::other("o 7z descompacta bytes demais"));
    }
    *total += escrito;
    Ok(())
}


#[cfg(test)]
mod testes {
    use super::*;
    use crate::loader::archive;
    use crate::scratch::TempDir;

    /// Escreve um `.7z` de verdade, para o teste não depender de arquivo no repositório.
    fn escreve_7z(caminho: &Path, entradas: &[(&str, &[u8])]) {
        let mut escritor = sevenz_rust2::ArchiveWriter::create(caminho).expect("criar 7z");
        for (nome, dados) in entradas {
            escritor
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file(nome),
                    Some(std::io::Cursor::new(dados.to_vec())),
                )
                .expect("escrever entrada");
        }
        escritor.finish().expect("fechar o 7z");
    }

    /// A assinatura decide, e não a extensão: um `.7z` chamado `.zip` continua sendo `.7z`.
    #[test]
    fn reconhece_pela_assinatura_e_nao_pela_extensao() {
        let pasta = TempDir::new("zeebx-7z-magia");
        let pacote = pasta.join("jogo.zip");
        escreve_7z(&pacote, &[("Titulo/mod/1/a.mod", b"mod")]);
        assert!(eh_sete_z(&pacote));
        std::fs::write(pasta.join("outro.zip"), b"PK\x03\x04nao e 7z").unwrap();
        assert!(!eh_sete_z(&pasta.join("outro.zip")));
    }

    /// O caminho inteiro do jogo funciona: achar o `.mod`, achar o `.mif` e extrair.
    ///
    /// É o que um dono de `.7z` precisa que funcione, e cada etapa passa por um lugar diferente do
    /// arquivo: a escolha do módulo vem do cabeçalho, o manifesto de uma entrada lida, e a
    /// extração do arquivo inteiro.
    #[test]
    fn acha_o_modulo_e_o_manifesto_dentro_do_7z() {
        let pasta = TempDir::new("zeebx-7z-jogo");
        let pacote = pasta.join("Jogo.7z");
        escreve_7z(
            &pacote,
            &[
                ("Jogo/leia-me.txt", b"texto"),
                ("Jogo/mod/279233/jogo.mod", b"codigo"),
                ("Jogo/mif/279233.mif", b"manifesto"),
            ],
        );

        assert_eq!(archive::find_module(&pacote).as_deref(), Some("Jogo/mod/279233/jogo.mod"));
        assert_eq!(
            archive::find_manifest(&pacote, "Jogo/mod/279233/jogo.mod").as_deref(),
            Some(&b"manifesto"[..])
        );

        let destino = pasta.join("extraido");
        std::fs::create_dir_all(&destino).unwrap();
        let limites = crate::loader::archive::ArchiveLimits::padrao();
        extrair(&pacote, &destino, limites).expect("extrai");
        assert_eq!(
            std::fs::read(destino.join("Jogo/mod/279233/jogo.mod")).unwrap(),
            b"codigo"
        );
    }

    /// Caminho com `..` é recusado, e nada é escrito fora do destino.
    #[test]
    fn recusa_caminho_que_sai_do_destino() {
        let pasta = TempDir::new("zeebx-7z-inseguro");
        let pacote = pasta.join("malicioso.7z");
        escreve_7z(&pacote, &[("../fora.txt", b"nao devia estar aqui")]);

        let destino = pasta.join("extraido");
        std::fs::create_dir_all(&destino).unwrap();
        let erro = extrair(&pacote, &destino, crate::loader::archive::ArchiveLimits::padrao())
            .expect_err("devia recusar");
        assert!(erro.to_string().contains("caminho inseguro"), "{erro}");
        assert!(!pasta.join("fora.txt").exists());
    }
}
