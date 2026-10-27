# 30 — Ajustes da repaginada

Ajustes depois das quatro fases ([27](27-moldura-da-janela.md), [28](28-jogo-na-janela-principal.md),
[29](29-configuracoes-com-barra-lateral.md)), pedidos depois de usar a interface: o ícone da
Z-Wheel, o pé da janela enquanto o jogo roda, o Z-Pad e o Dragon num botão só, a Z-Wheel e a procura
desabilitadas durante o jogo, a pergunta antes de fechar o jogo, e a classificação indicativa dos
detalhes dentro do diálogo.

## A Z-Wheel é a casa

O botão da Z-Wheel, na barra de ferramentas e no menu Arquivo, usava o `disc-3` da Lucide, um
disco, que não diz o que ela é. A Z-Wheel é a tela inicial do console: de lá se escolhe um jogo e se
começa. O ícone passou a ser o `house`, e o rótulo continua "Z-Wheel", que é o nome que o console
dá a ela. O `disc-3` não tinha outro uso e saiu do recorte da fonte, que continua com 33 ícones.

## O pé da janela durante o jogo

Com o jogo aberto, o pé da janela tinha uma faixa que não saía:

- **na janela separada**, uma linha de 24 px com o título e onde o jogo desenha, sem opção nenhuma
  para tirá-la;
- **na principal**, a barra de status, que só saía pelo menu Exibir, e aí saía da biblioteca também.
  Durante o jogo ela mostrava a contagem de jogos e a dica de teclas, que são da biblioteca.

Agora as duas saem do caminho do jogo:

- **A linha da janela separada não existe mais.** O título do jogo continua no título da janela
  ("Zeebx — Double Dragon — software").
- **A barra de status some enquanto o jogo roda na principal**, e o jogo desce até o pé da janela.
  Na biblioteca ela continua, e o Exibir → Barra de status continua valendo. Com a janela separada,
  a biblioteca segue à vista na principal, e a barra também, com o jogo à direita, como antes.
- **O painel de depuração continua como era**: a faixa de baixo, que encolhe o jogo, com as
  opções dele em Depuração.

### As métricas no canto

O que a barra e a linha diziam do jogo foi para uma caixa no canto de cima à direita, por cima do
quadro, ligada em Depuração → "Mostrar FPS e métricas no canto do jogo" (`debug.metricas`,
desligada por padrão). Ela não depende do painel e não tira espaço do jogo, para poder ficar ligada
jogando:

```
                                   31 FPS
                 103% da velocidade do console
                                    Software
```

Os quadros por segundo vêm em destaque; embaixo, a velocidade em relação ao console e onde o jogo
desenha: "Software", ou "Placa · 2× · llvmpipe", com a resolução interna e o nome da placa. A largura
é no máximo metade da do quadro, e o nome de uma placa comprido termina em reticências. As linhas
saem da mesma amostra da sessão que o painel e a barra de status usam (`Session::sample`), e o
núcleo as monta já traduzidas (`Nucleo::metricas`).

Os avisos do screenshot e do save state, que já moravam nesse canto, descem para baixo da caixa
quando ela está à mostra.

## Z-Pad e Dragon num botão só

Os dois controles têm os mesmos botões, os mesmos eixos e o mesmo desenho na página de controles;
muda a pegada. A escolha do aparelho tinha quatro botões (Z-Pad, Dragon, Boomerang, Teclado), e
agora tem três: **Controle**, Boomerang e Teclado. Com o Controle escolhido, uma caixa "Modelo" ao
lado escolhe entre o Z-Pad e o Dragon.

**O modelo não sumiu porque o console o vê.** Cada um anuncia o seu identificador USB (`1A5C:3033`
o Z-Pad, `1EAA:0135` o Dragon), e os jogos da Boomerang Sports mudam o tratamento por ele
([20](20-boomerang-e-wii-remote.md)). A configuração gravada não mudou: os mesmos `ZPad` e
`Controle` no `settings.json`, e os mesmos índices no `cfg.aparelho()`.

- Vindo do Boomerang ou do teclado, o Controle escolhe o Dragon, que é o padrão do emulador.
- Clicar no Controle já escolhido não troca o modelo.
- **Os três botões são exclusivos como botões de rádio** (`autoExclusive`). Sem isso, clicar no
  Controle já marcado o desmarcava: o Qt alterna o `checked` do botão no clique, e como o aparelho
  não mudava, a ligação não voltava a marcá-lo.

## A Z-Wheel e a procura esperam o jogo

Com o jogo na principal, o "Abrir a Z-Wheel" e o "Procurar jogos de novo" ficam desabilitados, no
menu Arquivo e na barra de ferramentas, como o pausar e o reiniciar ficam sem jogo. São coisas da
biblioteca, e o jogo está no lugar dela: abrir a Z-Wheel trocaria o jogo por ela sem pergunta
nenhuma. A dica do botão diz "Feche o jogo para abrir a Z-Wheel.". O botão HOME do controle já não
abria a Z-Wheel durante o jogo: a biblioteca não escuta o controle enquanto há um jogo aberto
(`Principal.sobreposta`).

Com a janela separada, a biblioteca continua à vista na principal, e os dois continuam valendo, como
antes: um jogo aberto por cima de outro fica na janela onde o anterior estava
([28](28-jogo-na-janela-principal.md)).

## A pergunta antes de fechar o jogo

Um Esc a mais jogava fora o que não estava salvo. Agora fechar o jogo pergunta antes, como o
"Confirm Shutdown" do PCSX2: "Fechar o jogo?", com "Fechar o jogo" e "Continuar jogando". Passam
pela pergunta:

- o Esc no jogo (só o primeiro aperto: segurado, o Esc repetido fecharia a pergunta que ele mesmo
  abriu);
- o Fechar do menu Sistema e o da barra de ferramentas;
- o fechar da janela separada;
- sair do Zeebx com um jogo aberto, pelo menu Arquivo ou fechando a principal ("Sair do Zeebx?").

**O jogo pausa enquanto a pergunta está aberta**, e desistir o devolve como estava: um jogo que já
estava pausado continua pausado. O Esc e o clique fora da pergunta desistem, então dois Esc seguidos
não fecham nada.

**A pergunta mora no palco, e não na janela principal** (`PalcoDoJogo.pedeParaFechar`), para
aparecer onde o jogo está: um diálogo da principal abriria atrás da janela separada. O Fechar da
principal com o jogo na janela separada traz essa janela para a frente. Quem pergunta é o palco;
quem fecha continua sendo o `fechaJogo()` da principal e o `fecha()` da janela separada, que já não
perguntam.

**Não perguntam:** a Z-Wheel, que é a tela inicial e não guarda nada que se perca, e o jogo que sai
sozinho. A opção "Perguntar antes de fechar o jogo", em Gráficos, ao lado da janela separada,
desliga tudo isso (`graphics.confirmar_ao_fechar`, ligada por padrão). Ela é global, como a escolha
da janela, e não muda por jogo.

## A classificação indicativa nos detalhes

Nos detalhes de um jogo (o botão 3 do controle, ou o C), a classificação indicativa é uma faixa
larga: a idade, o tema e o conteúdo, lado a lado. A imagem tinha só a altura dada (64 px), e ficava
com a largura do arquivo: passava da borda direita do diálogo, por cima da janela principal. Agora
ela tem a largura da coluna, e a altura segue a proporção, até os mesmos 64 px.

## Como se conferiu

No contêiner do Ubuntu 24.04, com o Qt 6.4.2 do `.deb`, numa tela virtual dirigida pelo `xdotool`:

- a casa na barra de ferramentas;
- o Bad Dudes na principal, com as métricas ligadas: sem a barra de status, o jogo até o pé da
  janela, a caixa no canto ("31 FPS · 103% · Software") e o aviso do F9 embaixo dela;
- o Bad Dudes na janela separada, com as métricas e o painel ligados: sem a linha de 24 px, o painel
  embaixo e a caixa em cima; a principal com a barra de status e o jogo à direita;
- os controles: Boomerang → Controle cai no Dragon; Z-Pad pela caixa; Controle de novo mantém o
  Z-Pad e continua marcado; Teclado grava `Teclado` no `settings.json`;
- a opção nova na página de Depuração, fora do grupo do painel.
- com o Bad Dudes na principal, a casa e a procura cinzas na barra e no menu Arquivo;
- o Esc perguntou, com o jogo pausado; o segundo Esc desistiu e o jogo voltou a andar; o Fechar da
  barra perguntou, e "Fechar o jogo" trouxe a biblioteca;
- o Sair do menu Arquivo, com o jogo aberto, perguntou "Sair do Zeebx?", e o "Sair" encerrou o
  processo;
- na janela separada, o Esc e o Fechar da barra da principal perguntaram dentro dela;
- a Z-Wheel, aberta pela casa, voltou à biblioteca no Esc sem perguntar;
- os detalhes do Bad Dudes com a classificação dentro do diálogo.

**Não conferido:** o fechar da janela separada pelo botão da moldura. A tela virtual não tem
gerenciador de janelas, e o caminho só foi visto pelo Esc, que chega à mesma pergunta.
