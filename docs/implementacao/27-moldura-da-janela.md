# 27 — A moldura da janela principal

A segunda fase da repaginada da interface, na linha do PCSX2 2.x: a janela da biblioteca ganha
barra de menus, barra de ferramentas e barra de status. **A grade e o slider ficam como eram**; o
que muda é o que vai em volta deles. A primeira fase foram os temas ([26](26-temas.md)); as
seguintes, o jogo desenhado dentro da janela principal ([28](28-jogo-na-janela-principal.md)) e as
configurações com uma barra lateral de categorias ([29](29-configuracoes-com-barra-lateral.md)).

A linha de botões soltos que havia em cima (Z-Wheel, busca, Saves, Configurações, Liberar sync) e a
linha da contagem com o "Procurar de novo" saíram: cada coisa foi para um menu, para a barra de
ferramentas ou para a de status.

## Os menus

```text
Arquivo        Abrir a Z-Wheel · Procurar jogos de novo · Sair
Sistema        Pausar/Continuar · Reiniciar · Fast-forward · Screenshot · Salvar estado ▸ ·
               Carregar estado ▸ · Desfazer carregar · Fechar o jogo
Configurações  Geral · Controles · Atalhos · Gráficos · Velocidade · Áudio · Depuração ·
               Configurações deste jogo…
Exibir         Grade · Slider · Barra de ferramentas · Barra de status · Tela cheia
Ferramentas    Saves… · Log da execução · Liberar a sincronização do Zeeboids
Ajuda          Repositório no GitHub · Servidor no Discord · Procurar atualização · Sobre o Zeebx
```

**A tecla ao lado do item é só texto.** O `shortcut` de um item de menu do QML vale na janela
inteira: o P digitado na busca pausaria o jogo. Quem atende as teclas continua sendo a janela do
jogo, e o item mostra o atalho que está gravado (o screenshot, o reiniciar e os do save state são
trocáveis, então o texto vem da aba Atalhos).

O menu Sistema age sobre o jogo aberto, que nesta fase ainda abre na janela dele. As ações passam
pelas mesmas funções do teclado da janela do jogo (`Jogo.pausa()`, `reinicia()`, `screenshot()`),
e o símbolo de pausa no meio da tela aparece dos dois jeitos. Sem jogo, o menu inteiro fica
desabilitado.

- **O fast-forward é um item marcável**, que liga e desliga. Um item de menu não se segura, e
  entregar ao atalho um "apertado" contínuo não serve: no modo de segurar ligaria, mas no de
  alternar viraria um toque só, e soltar não desligaria. A `Partida` tem um interruptor à parte
  (`alterna_o_avanco_fixo`), que vale por cima dos dois modos; o atalho continua como era.
- **O rewind não está no menu.** Voltar só faz sentido segurando, e ele continua no F8.
- **Salvar estado e Carregar estado** mostram os dez slots, com o tamanho de cada um ou "Vazio", e
  o Carregar desabilita os vazios. Escolher um slot o torna o atual, como no PCSX2: o F1 e o F3
  seguintes vão nele. O slot atual mostra a tecla. A lista é lida do disco quando o menu Sistema
  abre, e não a cada desenho.
- **Os submenus dos slots têm largura mínima.** O `Menu` do Qt Quick mede os itens na primeira vez
  que abre, e não cresceu quando um slot vazio passou a ocupado: o "Ocupado · 14.1 MB" encostava
  na tecla.

Os itens marcáveis (Grade, Slider, as barras, o log, o fast-forward) são relidos quando o menu
abre, e não amarrados: um clique num item marcável quebra a ligação dele com a configuração, e uma
troca feita nas configurações não apareceria no menu.

**Com um menu aberto, a biblioteca não escuta o controle**, como já não escutava com outra janela
por cima (`Principal.sobreposta`).

### Procurar atualização

Na abertura, a procura por versão nova só aparece quando acha uma. Pelo menu Ajuda, quem pediu quer
saber a resposta de qualquer jeito: "em dia" e a falha voltam num diálogo próprio
(`Nucleo::resultado_da_procura`), e a versão nova abre o aviso de sempre, com o instalador.

### Sobre

A aba Sobre das configurações virou o diálogo Ajuda > Sobre o Zeebx, com a logo, a versão e os
dois endereços. O Android continua com a tela Sobre dele.

## A barra de ferramentas

Z-Wheel e Procurar de novo; Pausar, Reiniciar e Fechar, habilitados com o jogo aberto;
Configurações e Controles; Tela cheia (a do jogo, se ele está aberto, e a da biblioteca, se não). À
direita, a grade e o slider, como o par grade/lista do PCSX2, e a busca, com o ícone de lupa e o
botão de limpar dentro do campo.

**Os botões não pegam o foco do teclado**, como os de antes: clicado, um botão do Qt Quick fica
com ele, e as setas deixavam de ir para a grade.

Com a janela mais estreita que 1200 pontos, os rótulos somem e o nome vai para a dica. A largura é
a que os rótulos em português pedem, os mais longos dos quatro idiomas.

### Os ícones

Os ícones são da [Lucide](https://lucide.dev), desenhados como letras de uma fonte, e não como SVG:
a cor é a do texto, e acompanha o tema sem imagem para recolorir; e o Qt não precisa do módulo
QtSvg, que o `.deb`, o `windeployqt` e o `macdeployqt` teriam de levar junto.

A fonte inteira tem 2150 ícones e 900 KB. O [`ferramentas/icones.py`](../../ferramentas/icones.py)
recorta só os que a interface usa (33, com 13 KB, em `assets/icones/lucide.ttf`, embutida no binário)
e escreve a tabela de nomes do `Icone.qml`. Para acrescentar um ícone, o nome entra na lista do
script e ele roda de novo, sobre o pacote `lucide-static` da mesma versão. A licença é ISC, e MIT
para os ícones que vieram do Feather; as duas estão em `ferramentas/licencas/lucide-LICENSE` e no
`THIRD-PARTY-NOTICES.txt`.

## A barra de status

À esquerda, a contagem de jogos e a dica de teclas que antes ficava na segunda linha. Com um jogo
aberto, à direita, o nome dele, onde ele desenha (a placa ou o processador) e a velocidade da
última amostra, a mesma do painel de depuração: "Double Dragon — software · 100% · 60 FPS".

As duas barras se escondem pelo menu Exibir, e a escolha fica gravada (`barras` no
`settings.json`). Depois das fases, a de status passou a sumir enquanto o jogo roda na principal, e
o que ela dizia dele foi para a caixa de métricas do canto ([30](30-ajustes-da-repaginada.md)).

## Como se conferiu

Num contêiner com o Ubuntu 24.04 e o Qt 6.4.2, o do `.deb`: o `zeebx-qt` aberto numa tela virtual
(Xvfb), dirigido pelo `xdotool`, com uma foto a cada passo. Com o Double Dragon aberto, o menu
Sistema habilitou as ações, salvar no slot 3 pelo submenu gravou o arquivo e mostrou o aviso no
jogo, o Carregar estado deixou escolher só o slot 3, carregar mostrou "Estado do slot 3 carregado.
F4 desfaz.", e a barra de status mostrou o jogo e a velocidade. A moldura também foi vista no tema
Laranja e no Escuro, e a barra de ferramentas estreita, só com os ícones.
