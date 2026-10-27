# 28 — O jogo na janela principal

A terceira fase da repaginada da interface. Até a 0.4.3 o jogo abria sempre numa janela só dele,
ao lado da biblioteca. Agora, como no PCSX2, ele é desenhado na própria janela principal, no lugar
da biblioteca, com a barra de menus, a de ferramentas e a de status em volta ([27](27-moldura-da-janela.md)).
Fechar o jogo traz a biblioteca de volta. A janela separada continua existindo: é a opção
Configurações > Gráficos > "Jogo numa janela separada" (`graphics.jogo_separado`), o "Render to
Separate Window" do PCSX2.

## Um palco, duas casas

O que era a `Jogo.qml` inteira (o quadro, os avisos por cima dele, o símbolo da pausa, o painel de
depuração, o teclado e o passo a cada quadro) saiu para o `PalcoDoJogo.qml`, um componente sem
janela. A principal tem um palco, e a janela separada tem outro. **Só o palco de onde o jogo está
anda**: o `FrameAnimation` de cada um roda com o `ativo` dele. O núcleo é um só, e com os dois
palcos ativos ele andaria duas vezes por quadro.

Por isso, **onde o jogo aparece só se decide sem jogo rodando**. Um jogo aberto por cima de outro,
pela biblioteca com a janela separada ou pela Z-Wheel, fica onde o anterior estava. A opção vale no
próximo jogo.

O que é da janela, como fechar e pôr em tela cheia, sai do palco como sinal (`fechaPedido`,
`telaCheiaPedida`), e quem hospeda decide o que fazer. O menu Sistema, a barra de ferramentas e a
de status da principal falam com o palco ativo (`Principal.palco`), e funcionam com o jogo em
qualquer uma das duas.

**Dois palcos, e não um que muda de janela.** Um item do Qt Quick pode trocar de janela pelo
`parent`, mas o `ItemDoQuadro` guarda a textura do quadro no scene graph da janela onde está, e a
troca teria de refazer esse nó no meio do jogo. Com dois palcos não há troca nenhuma: cada um vive
na sua janela, e o que fica parado não desenha.

## Na principal

- **A biblioteca sai e o palco entra**, ocupando a área inteira, sem as margens da biblioteca. A
  busca e os botões de grade e slider somem da barra de ferramentas, e o Ctrl+F não leva à busca:
  durante o jogo, as letras são do jogo.
- **A janela fica no modo configurado para o jogo** (`graphics.janela_do_jogo`: em janela,
  maximizada ou em tela cheia) enquanto ele roda, e volta ao que era quando ele fecha. O modo da
  própria principal, trocado nas configurações durante o jogo, espera o jogo fechar.
- **Em tela cheia, as três barras saem**, como no PCSX2: a tela é do jogo. O F11 e o Alt+Enter
  alternam.
- **A linha de estado embaixo do quadro não existe**: quem diz o título e onde o jogo desenha é a
  barra de status. O título da janela vira "Zeebx — Double Dragon — software". Depois, a barra de
  status passou a sumir durante o jogo, e a linha saiu também da janela separada
  ([30](30-ajustes-da-repaginada.md)).
- **O log da execução** fica por cima da janela onde o jogo está.
- Perder o foco solta as teclas, como na janela separada.

## A textura em outra janela

Pela placa, o quadro é uma textura do contexto de GL próprio do rasterizador, e o `ItemDoQuadro` a
mostra sem voltar à CPU ([21](21-migracao-para-qt.md)). O contexto é compartilhado com o do Qt
Quick (`AA_ShareOpenGLContexts`), e por isso a mesma textura serve à principal como servia à janela
separada. Conferido com o Alien Breaker, que desenha em 3D, pela placa do Mesa (llvmpipe).

## Quem vem da 0.4.x

A opção nova não existe no `settings.json` antigo, e o padrão dela é o jogo na principal: quem
atualiza vê a repaginada, e a janela separada está a um clique. O `graphics.janela_do_jogo`
continua valendo, agora para a principal enquanto o jogo roda.

## Como se conferiu

No contêiner do Ubuntu 24.04, com o Qt 6.4.2 do `.deb`, numa tela virtual dirigida pelo `xdotool`:

- o Double Dragon aberto pela grade aparece na principal, o P pausa (o símbolo aparece e o botão
  vira "Continuar") e o Esc traz a biblioteca de volta, com a busca e os botões de vista;
- o Alien Breaker, pela placa, aparece na principal desenhado pelo llvmpipe; o F11 tira as três
  barras e o segundo F11 as devolve;
- com a janela separada ligada, o jogo abre na janela dele, com a linha de estado embaixo; o menu
  Sistema da principal acompanha a pausa dele, e o Esc na janela do jogo a fecha.
