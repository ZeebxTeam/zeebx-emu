# 29 — As configurações com barra lateral

A quarta fase da repaginada da interface, e a última do plano. A janela de configurações troca as
abas no topo por uma coluna de categorias com ícones, à esquerda, e a explicação das opções sai de
baixo de cada uma para um painel no rodapé, como no PCSX2. As páginas em si (as opções, a ordem, o
que cada uma grava) não mudaram.

## A barra lateral

Geral, Controles, Atalhos, Gráficos, Velocidade, Áudio e Depuração, cada uma com um ícone da
Lucide ([27](27-moldura-da-janela.md#os-ícones)): `settings`, `gamepad-2`, `keyboard`, `monitor`,
`gauge`, `volume-2` e `bug`. Os mesmos ícones foram para o menu Configurações da janela principal.

**A coluna mantém o `id: abas` e o `currentIndex` da `TabBar` de antes.** Quem abre a janela numa
categoria (`abre(1)`, os itens do menu Configurações) e as páginas que leem o controle enquanto estão
à vista (`abas.currentIndex === 1`, a de controles e a de atalhos) perguntam por eles, e continuaram
iguais.

As categorias não pegam o foco do teclado no clique, como os botões da barra de ferramentas: a
captura de teclas das páginas de controles e de atalhos precisa dele.

No modo de um jogo só (as configurações próprias de um jogo), as categorias que não valem por jogo
(Geral, Atalhos e Depuração) somem, e a coluna mostra só Controles, Gráficos, Velocidade e Áudio.

## A explicação no rodapé

Embaixo de cada opção, as explicações alongavam as páginas até elas não caberem na janela. Agora o
rodapé mostra a da opção sob o ponteiro, ou com o foco do teclado: quem anda com o Tab lê o que cada
opção faz. Sem nada para explicar, ele diz como fazê-lo aparecer. A altura é fixa: uma que seguisse
o texto faria a janela pular a cada opção sobrevoada.

**O que muda com o estado continua junto da opção**: a porta desligada, a captura esperando uma
tecla, nenhum controle encontrado, a calibração em andamento. Isso é aviso, e não explicação, e
precisa ser visto sem procurar.

### Como a página acha a explicação

Três marcadores dizem o que explicar:

- o **`Titulo`** de um grupo de opções leva a `descricao` do grupo, que vale do título até o
  próximo. As dicas fixas que vinham logo abaixo de um título viraram a `descricao` dele;
- a **`Opcao`** (um liga-desliga) leva a `dica` dela, que antes ficava embaixo, apagada;
- a **`Explica`** não ocupa lugar e explica o que está logo acima dela: uma caixa de escolha, uma
  linha de botões. As dicas fixas que vinham depois de um controle, e não de um título, viraram
  `Explica`.

Cada página (`Pagina`) tem um `HoverHandler`. Com o ponteiro na altura `y`, ela pega o marcador mais
próximo acima dele (`descreve`). Uma opção sem dica mostra a do título do grupo dela. Acima do
primeiro título vale a `descricaoDaPagina`: a das páginas que abriam com uma explicação geral
(Atalhos, Gráficos, Áudio e Depuração). Trocar de página apaga a explicação da anterior.

**Só os filhos diretos da página entram.** As dicas fixas que moram dentro de um grupo aninhado
(a resolução interna e o antialias, dentro do grupo do rasterizador na placa; a memória do rewind;
o turbo e os eixos, na página de controles) continuam embaixo da opção. Movê-las pediria que cada
grupo também fosse uma página, e ficou para depois.

A lista de filhos de um item não é iterável com `for...of` no Qt 6.4 do Ubuntu 24.04: o laço dava
`TypeError` e o rodapé ficava parado. O `descreve` anda por índice.

## A janela

Maior, para caber a coluna: 900 por 660, e no mínimo 600 por 400 (era 720 por 640, e 420 por 320).

## Como se conferiu

No contêiner do Ubuntu 24.04, com o Qt 6.4.2, numa tela virtual dirigida pelo `xdotool`: na página
Geral, o ponteiro sobre o seletor de tema trouxe a explicação do tema, e sobre "Z-Wheel de fim de
vida", a dela; na Velocidade, sobre o pular quadros, a do pular quadros, e sobre o rewind, a dele;
nos Atalhos, a da página. Aberta pelo X na biblioteca, a janela de configurações de um jogo mostrou
só as quatro categorias por jogo, com a opção de usar as configurações próprias em cima.
