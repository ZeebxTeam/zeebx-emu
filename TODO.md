# Zeebx - próximos passos

## Diagnósticos de CPU no Dynarmic

As opções abaixo dependiam dos hooks de instrução e memória do Unicorn. Elas foram removidas da
linha de comando junto com o backend antigo, em vez de permanecerem aceitas sem produzir dados.
Precisam ser redesenhadas sobre as APIs de instrumentação e invalidação de blocos do Dynarmic:

- **`--code=0xINI:0xFIM`** registrava cada instrução executada dentro de uma faixa, acompanhada
  de `r0` e `lr`. Servia para transformar a desmontagem, que mostra todos os caminhos possíveis,
  no caminho que o jogo realmente percorreu.
- **`--watch=0xADDR`** observava leituras e escritas ao redor de um endereço e registrava o PC,
  o `lr`, o tamanho e o valor de cada acesso. Servia para descobrir quem inicializou, alterou ou
  apenas leu um campo corrompido ou inesperadamente nulo. Não se confunde com `watch_dirty`, que
  continua no backend para invalidação interna das superfícies do renderizador.
- **`--profile`** contava os blocos ARM mais executados e media o tempo gasto nos métodos da API
  BREW. Servia para separar laços quentes do jogo de gargalos no despachante do emulador e orientar
  otimizações com medidas.
- **`--wall=SEGUNDOS`** interrompia a execução depois de um limite de tempo real. Servia como
  proteção em diagnósticos e automações quando o relógio virtual ou o limite de instruções não
  encerravam rapidamente um jogo preso.

O **`--trace[=filtro]` não faz parte desta pendência**: ele rastreia chamadas BREW na `Machine`,
não instruções do backend, e continua funcionando com Dynarmic.

## Recursos

- Save State
- Fast-Forward
- Modo Turbo
- Rewind
- Cheats
