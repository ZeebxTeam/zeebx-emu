#ifndef ZEEBX_IOS_H
#define ZEEBX_IOS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* O que `zeebx_ios_passo` devolve. Os números são o contrato com o Swift. */
#define ZEEBX_SEM_SESSAO (-1)
#define ZEEBX_QUADRO 0
#define ZEEBX_RODANDO 1
#define ZEEBX_ADIANTADO 2
#define ZEEBX_PAROU 3

/* Ponteiro opaco. A Swift não olha para dentro. */
typedef struct ZeebxIos ZeebxIos;

/*
 * `documentos` é a pasta Documents do aplicativo: as ROMs ficam em `documentos/roms`,
 * que o app de Arquivos enxerga. `suporte` é Library/Application Support: cache, saves
 * e o aparelho virtual. Os dois são caminhos de arquivo, em UTF-8, terminados em zero.
 * Devolve nulo se um dos ponteiros for nulo.
 */
ZeebxIos *zeebx_ios_cria(const char *documentos, const char *suporte);
void zeebx_ios_destroi(ZeebxIos *app);

/* Quantos jogos a última varredura achou. Varre de novo a pasta de ROMs. */
int32_t zeebx_ios_varre(ZeebxIos *app);

/* Título do jogo `indice`, válido até a próxima varredura. Nulo se o índice não existe. */
const char *zeebx_ios_titulo(ZeebxIos *app, int32_t indice);

/*
 * Começa o jogo `indice`. 0 quando a sessão subiu; -1 quando não, e a razão fica em
 * `zeebx_ios_erro` até a próxima chamada que a substitua.
 */
int32_t zeebx_ios_comeca(ZeebxIos *app, int32_t indice);
void zeebx_ios_para(ZeebxIos *app);

/*
 * Uma volta do laço: entrega o controle, avança o jogo o tempo real que passou e publica
 * o quadro. A chamada é da linha de execução da interface; não é segura de outra linha.
 */
int32_t zeebx_ios_passo(ZeebxIos *app);

/*
 * O quadro corrente, em BGRA8, com a origem no canto superior esquerdo. O ponteiro vale
 * até o próximo `zeebx_ios_passo` ou `zeebx_ios_para`. `largura` e `altura` podem ser nulos.
 * Devolve nulo quando não há quadro.
 */
const uint8_t *zeebx_ios_quadro(ZeebxIos *app, uint32_t *largura, uint32_t *altura);

/* `indice` é a posição em `BUTTON_NAMES` do núcleo (`b1` é 0, `up` é 12). `apertado` é 0 ou 1. */
void zeebx_ios_botao(ZeebxIos *app, uint32_t indice, int32_t apertado);

/* Eixo do manche, centrado em zero. `indice` 0 é X, 1 é Y. O curso é -128..=128. */
void zeebx_ios_eixo(ZeebxIos *app, uint32_t indice, int32_t valor);

/* -1 quando o nome não é botão do Zeebo. */
int32_t zeebx_ios_botao_por_nome(const char *nome);

/* Texto da última falha, ou string vazia. Vale até a próxima chamada que o substitua. */
const char *zeebx_ios_erro(ZeebxIos *app);

#ifdef __cplusplus
}
#endif

#endif
