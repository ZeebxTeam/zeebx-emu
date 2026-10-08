//! Remendos de ritmo por jogo, aplicados aos bytes do módulo antes da análise.
//!
//! O modelo é o GameDB do PCSX2: cada entrada diz o jogo (pela classe do applet), os bytes
//! esperados e os bytes novos, e o emulador remenda a cópia em memória ao carregar — o arquivo
//! do jogador não muda. Só entra aqui o que foi medido e decodificado: remendo sem medida é
//! palpite com endereço.
//!
//! A primeira entrada é o regulador de quadro do Resident Evil 4 (patch do modder Jailson,
//! decodificado na issue #81): o jogo calcula `espera = orçamento − decorrido` e emenda
//! `espera = max(espera, 10)` antes de esperar — num aparelho que já estoura o orçamento, são
//! 10 ms extras por quadro de câmera lenta. Com 1, o quadro estourado espera 1 ms. O resto do
//! pacote do modder (desvio e carimbo) não entra: o desvio não foi totalmente decodificado, e
//! o carimbo é metadado do pacote, não comportamento.

/// Um remendo: onde, no arquivo do `.mod`, o que tem de estar lá e o que entra no lugar.
///
/// Deslocamentos de **arquivo**, não endereços do guest: o remendo acontece nos bytes lidos,
/// antes da análise — sem matemática de endereço de carga.
struct Remendo {
    deslocamento: usize,
    esperado: &'static [u8],
    novo: &'static [u8],
}

/// Um jogo remendado: a classe, o nome para o registro e o porquê documentado.
struct Jogo {
    classe: u32,
    nome: &'static str,
    motivo: &'static str,
    remendos: &'static [Remendo],
}

/// A base inteira. Uma entrada por jogo, e cada uma com o motivo medido.
static JOGOS: &[Jogo] = &[
    Jogo {
        // A classe BREW do applet (a do `applet_clsid`), e não o número da pasta do pacote:
        // o RE4 mora em `mod/276675`, mas se apresenta como 0x0108af6c.
        classe: 0x0108af6c,
        nome: "Resident Evil 4",
        motivo: "espera mínima do regulador de 10 ms para 1 ms (issue #81)",
        remendos: &[
            // Deslocamentos de arquivo (o `cmp` conta de 1): os dois `0x0a` do regulador.
            Remendo {
                deslocamento: 179136,
                esperado: &[0x0a],
                novo: &[0x01],
            },
            Remendo {
                deslocamento: 179144,
                esperado: &[0x0a],
                novo: &[0x01],
            },
        ],
    },
];

/// Aplica os remendos do jogo aos bytes do módulo, e devolve quantos pegaram.
///
/// Tudo-ou-nada por jogo: se qualquer esperado não confere (outra versão do módulo, por
/// exemplo), **nenhum** remendo daquele jogo entra — meio remendo é o pior dos mundos, porque
/// o jogo roda diferente do original e do remendado. O que não pegou sai no registro, e o
/// jogo segue sem remendo.
///
/// `ZEEBX_PATCH=0` desliga tudo: é o interruptor de emergência para um remendo que se
/// comporte mal numa versão não testada, e segue a mesma convenção do `ZEEBX_GPU`.
pub fn aplica_para(classe: Option<u32>, mut bytes: Vec<u8>) -> Vec<u8> {
    if matches!(std::env::var("ZEEBX_PATCH").as_deref(), Ok("0")) {
        crate::registro!(
            crate::registro::Nivel::Depuracao,
            "patch",
            "remendos desligados pelo ambiente (ZEEBX_PATCH=0)"
        );
        return bytes;
    }
    let classe = match classe {
        Some(c) => c,
        None => return bytes,
    };
    let Some(jogo) = JOGOS.iter().find(|j| j.classe == classe) else {
        return bytes;
    };
    let confere = jogo.remendos.iter().all(|r| {
        bytes
            .get(r.deslocamento..r.deslocamento + r.esperado.len())
            .is_some_and(|trecho| trecho == r.esperado)
    });
    if !confere {
        crate::registro!(
            crate::registro::Nivel::Aviso,
            "patch",
            "{}: bytes diferentes do esperado, remendo não aplicado ({})",
            jogo.nome,
            jogo.motivo,
        );
        return bytes;
    }
    for r in jogo.remendos {
        bytes[r.deslocamento..r.deslocamento + r.novo.len()].copy_from_slice(r.novo);
    }
    crate::registro!(
        crate::registro::Nivel::Informacao,
        "patch",
        "{}: {} remendo(s) aplicado(s) ({})",
        jogo.nome,
        jogo.remendos.len(),
        jogo.motivo,
    );
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Os bytes do RE4 original nos dois pontos do regulador, e nada mais.
    fn modulo_re4() -> Vec<u8> {
        let mut bytes = vec![0u8; 180000];
        bytes[179136] = 0x0a;
        bytes[179144] = 0x0a;
        bytes
    }

    #[test]
    fn o_remendo_do_re4_troca_os_dois_10_por_1() {
        let remendado = aplica_para(Some(0x0108af6c), modulo_re4());
        assert_eq!(remendado[179136], 0x01);
        assert_eq!(remendado[179144], 0x01);
    }

    #[test]
    fn bytes_diferentes_anulam_o_jogo_inteiro() {
        let mut bytes = modulo_re4();
        bytes[179144] = 0x05;
        let saida = aplica_para(Some(0x0108af6c), bytes.clone());
        assert_eq!(saida, bytes, "um esperado furado não pode aplicar o outro");
    }

    #[test]
    fn classe_desconhecida_passa_direto() {
        let bytes = modulo_re4();
        let saida = aplica_para(Some(1), bytes.clone());
        assert_eq!(saida, bytes);
    }

    #[test]
    fn sem_classe_passa_direto() {
        let bytes = modulo_re4();
        let saida = aplica_para(None, bytes.clone());
        assert_eq!(saida, bytes);
    }
}
