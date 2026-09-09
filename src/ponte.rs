//! Pontes por módulo: o pouco de conhecimento específico de um jogo que o emulador precisa,
//! isolado aqui e em lugar nenhum mais.
//!
//! **Isto não é emulação.** Todo o resto do Zeebx implementa API do BREW ou lê vtable do
//! console — coisas que valem para qualquer título. O que está neste arquivo vale para **um**
//! jogo, e por isso mora separado, é opcional, e tem o nome do que é.
//!
//! # Por que existe
//!
//! Para estudar um protocolo de rede é preciso que o jogo **consuma** a resposta do servidor, e
//! não só que ela chegue. No Zeeboids a resposta vira um vetor de strings dentro de um objeto do
//! próprio jogo, e essas strings têm de sair do alocador dele: entregar memória de fora faz o
//! gerenciador reclamar quando vai liberá-las (`TTDMemoryManager.cpp:916`). No console quem
//! preenchia esse vetor era o firmware, que naturalmente usava o alocador certo.
//!
//! # A regra que a ponte não pode quebrar
//!
//! Ela **não toca no que trafega**. Não muda a requisição, não muda a resposta, não existe do
//! lado do servidor. Um Zeebo de verdade falando com o mesmo servidor vê exatamente o mesmo
//! HTTP — é essa a razão de a ponte mexer só em como a resposta é depositada na memória do
//! jogo, e não no conteúdo dela.
//!
//! O corolário importa tanto quanto a regra: **a ponte nunca justifica mudar o servidor.** Se
//! ajustássemos a resposta até o nosso jeito de entregar aceitá-la, acertaríamos aqui e
//! erraríamos no console. O formato se decide pelo que o jogo faz, e a prova final é o aparelho
//! de verdade.

//! # Estado: incompleta, e por isso desligada
//!
//! Ela derrubou o jogo num teste de exportação: acesso inválido a `0xe4`, vindo do gerenciador
//! de memória. A causa aparente é que o vetor guarda **objetos `ttdString`**, e não `char *` —
//! o construtor em `0xa85e0` grava o comprimento em `[obj]` antes de alocar o texto, então o
//! objeto é `{ comprimento, ponteiro }`. Entregando texto cru, o destrutor lê `[ponteiro+4]`
//! como endereço e libera lixo.
//!
//! Uma entrega errada não falha na hora: corrompe e quebra adiante, que é o pior tipo de erro
//! para se ter ligado por omissão. Fica atrás do `--ponte` (ou de `ZEEBX_PONTE=1`, que alcança
//! a interface), e o padrão é o jogo ver "não veio resposta" — um estado que ele sabe tratar.
//!
//! # Onde a investigação parou
//!
//! A queda foi reproduzida sem janela, o que é a parte boa: `str r0, [r5]` em `0x654ac` com
//! `r5` valendo `0x1c`, dentro do gerenciador de memória, com a pilha ainda nos quadros da
//! `ConnectionManager::init` — ou seja, **dentro da chamada que nós fazemos**.
//!
//! O que já foi descartado: o alocador é chamado como o jogo o chama (índice de pool 0, que ele
//! exige menor que 32; o quinto argumento na pilha, que ele lê em `[sp+0x30]`; o ponteiro de
//! arquivo para rastreio). E o objeto de resposta é mesmo o desserializador — contagem em `+8`,
//! vetor em `+4`, cursor em `+0x28`, tipo em `+0x2c` —, o que o iterador em `0xa583c` confirma.
//!
//! Era **o momento**, e está resolvido: a resposta espera numa fila e é depositada na fronteira de
//! chamada, a mesma que os sinais usam, quando o guest não está dentro de nada. Chamar o
//! alocador de dentro do despacho reentrava num gerenciador que estava no meio de uma operação.
//!
//! Com isso a entrega funciona: os campos são escritos, as guardas passam e o jogo não quebra.
//! O que decide o resto é o **conteúdo** da resposta, que é o que as variantes do servidor
//! existem para medir.

/// O que se sabe de um módulo específico.
#[derive(Debug, Clone, Copy)]
pub struct Ponte {
    /// O `malloc` do gerenciador de memória do próprio jogo.
    ///
    /// Assinatura observada: `(tamanho, 0, linha, arquivo, 1)` — os dois últimos são rastreio de
    /// origem, que ele guarda para os relatórios dele.
    pub alocador: u32,
    /// Um ponteiro para nome de arquivo, no próprio módulo, para o campo de rastreio.
    pub origem: u32,
}

/// O Zeeboids (`ClassID` do `.mif`).
const ZEEBOIDS: u32 = 0x0108_ff1a;

/// A ponte do módulo, se houver.
///
/// Os endereços saíram da desmontagem do `zeeboids.mod` da versão 1.1.1402: o construtor de
/// string em `0xa85e0` chama o alocador em `0x6d440` passando `'source\ttdString.cpp'`, que
/// está em `0xa8648`. Outra versão do jogo terá outros endereços, e é por isso que a ponte é
/// por módulo e não uma constante solta.
pub fn para(clsid: u32) -> Option<Ponte> {
    match clsid {
        ZEEBOIDS => Some(Ponte {
            alocador: 0x0006_d440,
            origem: 0x000a_8648,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_o_modulo_declarado_tem_ponte() {
        assert!(para(ZEEBOIDS).is_some());
        // Qualquer outro jogo não ganha tratamento especial nenhum: é o ponto do arquivo.
        assert!(para(0x0107_0798).is_none());
        assert!(para(0).is_none());
    }
}
