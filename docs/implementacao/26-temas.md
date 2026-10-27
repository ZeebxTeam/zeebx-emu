# 26 — Temas

A interface Qt tem temas de cor: Sistema (o padrão, a paleta do sistema operacional), Claro,
Escuro, AMOLED, Laranja, Azul, Rubi e Esmeralda. Escolhe-se em Configurações > Geral > Tema, e a
troca vale na hora, em todas as janelas. É a primeira fase da repaginada da interface, que segue a
linha do PCSX2.

## Paleta, e não estilo

O Qt Quick tem dois jeitos de mudar a aparência. **O estilo** troca o desenho dos controles
(Fusion, Material, Universal, Imagine, FluentWinUI3). **A paleta** troca só as cores de um estilo.
Os temas do PCSX2 são paletas sobre o Fusion, e os do Zeebx também. O estilo ficou de fora por
quatro motivos:

- o estilo é fixado quando o programa abre, e trocá-lo pede reiniciar;
- o Material e o Universal tiram as cores das propriedades deles (`Material.theme`,
  `Material.accent`), e não da paleta. A grade e o slider da biblioteca são desenhados à mão com
  `palette.*`, e ficariam com cores que não combinam com o resto;
- o FluentWinUI3 só existe a partir do Qt 6.8, e o `.deb` usa o Qt 6.4 do Ubuntu 24.04;
- cada estilo novo teria de ir no AppImage, no `.exe` e no `.app`, e ser conferido nos três.

O Fusion continua sendo o estilo (`gl_qt.cpp`), pelo motivo de antes: ele segue a paleta em todos
os controles, e o Basic pintava texto escuro sobre fundo escuro.

## O arquivo

```text
assets/temas/<chave>.json      os de fábrica, embutidos no binário
<pasta do executável>/temas/   uma cópia portátil
<config>/temas/                os de quem quiser acrescentar
```

O formato 1 tem `nome` por idioma, `escuro`, `cores` com os vinte papéis do `QPalette` (com os
nomes que o QML usa: `window`, `windowText`, `button`...), `desabilitado` com o texto dos controles
desabilitados, e `aviso` e `erro`, as duas cores que a QML escrevia à mão (`#e0a030` e `#d04040`).
O leitor é o [`src/ui/temas.rs`](../../src/ui/temas.rs).

**Todos os papéis são obrigatórios.** Um papel que faltasse ficaria com a cor do sistema, e um texto
claro do tema por cima de um fundo claro do sistema é ilegível. Um arquivo com papel faltando, cor
que não é `#rrggbb` ou formato desconhecido é ignorado, com o motivo no registro; o programa abre
com os outros, e quem o tinha escolhido fica na paleta do sistema até o arquivo voltar a valer. Um
arquivo com a chave de um de fábrica toma o lugar dele.

A escolha é gravada pela chave (`"tema": "rubi"` no `settings.json`); `null` é o Sistema.

## Como a paleta chega às janelas

O cxx-qt-lib não tem o `QPalette`, então a montagem mora em C++
([`cpp/paleta.cpp`](../../frontends/qt-standalone/src/qt/cpp/paleta.cpp)). O Rust passa cor por cor
e a aplicação é do programa inteiro, pelo `QGuiApplication::setPalette`. Para voltar ao Sistema,
`setPalette(QPalette())`. No Qt 6.8 ou mais novo o `styleHints()->setColorScheme` também diz ao
sistema se o tema é escuro, para a barra de título acompanhar onde ele sabe fazer isso.

Três coisas foram medidas, porque nenhuma é óbvia pela documentação:

- **As janelas acompanham a troca com o programa aberto**: o fundo, os botões, os campos, os
  rótulos e os `Rectangle` amarrados a `palette.*`. Conferido num programa de teste no Qt 6.12 e
  no Qt 6.4.2 do Ubuntu 24.04, e depois no `zeebx-qt`, trocando o tema pelo seletor numa tela
  virtual.
- **O tema é aplicado depois do `engine.load`**, e não antes. Aplicado antes, só os itens soltos (os
  cartões da grade) o seguiam: as janelas e os controles do Fusion nasciam com a paleta do sistema.
  Eles só passam à do programa quando ela muda com eles já criados. Depois do `load` as janelas
  existem e ainda não foram mostradas, então ninguém vê a troca.
- **Fora do `nucleo::com`.** O `setPalette` avisa as janelas na hora, as ligações do QML se refazem
  ali dentro, e uma que chame o núcleo encontraria o `RefCell` emprestado. O tema é copiado lá
  dentro e aplicado fora (`tema::aplica`).

As cores de aviso e de erro vêm do objeto `Tema` do QML, relido quando o tema muda
(`Configuracoes.temaMudou`). A janela do jogo não segue o tema: o fundo é preto, e os avisos por
cima do jogo continuam escuros, porque são lidos sobre o jogo, e não sobre a interface.

### As dicas

As dicas da interface trocam o conteúdo do `ToolTip` por um `Label` próprio, e um `Label` pinta com
`windowText`. Com a paleta do sistema isso nunca apareceu, porque a dica amarela e o texto escuro da
janela combinam. No tema Claro, de dica escura e janela clara, o texto da dica sumia. Os `Label` das
dicas pintam com `palette.toolTipText`.

## O contraste

O [`ferramentas/confere_temas.py`](../../ferramentas/confere_temas.py) confere o formato e o
contraste dos pares que a interface desenha de verdade, pela regra do WCAG 2.1: 4,5:1 para texto e
3:1 para bordas e para o texto de exemplo dos campos. Entre os pares estão o título do cartão
escolhido (o `highlight` a 16% por cima do `button`) e a borda da caixa escolhida do slider contra o
painel claro fixo dela (`#ececec`). A lista de papéis do script tem de andar junto com a `PAPEIS` do
`temas.rs`.

```bash
python3 ferramentas/confere_temas.py                 # os de fábrica
python3 ferramentas/confere_temas.py meu-tema.json   # um tema qualquer
```

Nos temas escuros, o destaque cabe numa faixa estreita: escuro o bastante para o texto por cima e
claro o bastante para a borda aparecer contra a janela e contra o painel do slider. Os destaques
foram achados variando só a luminosidade, no tom e na saturação escolhidos.

- **No AMOLED**, os campos e os botões têm fundo próprio. A borda que o Fusion desenha é a cor da
  janela escurecida (`darker(140)`), e com a janela preta ela também sai preta e some.
- **O Laranja e o Azul vêm do Zeebo.** O destaque do Laranja é o laranja da caixa do console
  (`#cd4114` numa foto; `#ca4014` no tema). O do Azul tem o tom e a saturação do logotipo aceso no
  console (225°), com a luminosidade que o texto branco por cima pede.

## O que ficou de fora

- A escolha do estilo, pelos motivos lá em cima.
- Uma cor de destaque separada do tema. Cada tema traz o seu, combinado com as outras cores; quem
  quer outro faz um tema.
