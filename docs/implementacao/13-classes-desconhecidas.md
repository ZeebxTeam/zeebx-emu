# 13 — Classes desconhecidas: como descobrir o que elas são

O BREW identifica cada classe por um número. Quando um jogo pede um número que não conhecemos, a
resposta certa é `ECLASSNOTSUPPORT` — e o jogo em geral desiste. A pergunta que sobra é sempre a
mesma: **que interface é essa?**

Sem o header, o caminho tradicional é adivinhar pelo nome vizinho e testar. Foi assim que o
`IHID` do console foi identificado, e custou caro. O `--sonda` faz isso virar procedimento.

## O que a sonda faz

`--sonda=0xCLSID[,0xCLSID…]` manda atender a classe com um objeto de observação em vez de
recusá-la. Esse objeto não implementa interface nenhuma: ele **registra** cada chamada — o slot,
os argumentos e o texto de qualquer argumento que aponte para texto — e responde `SUCCESS`.

Três decisões fazem a diferença entre uma ferramenta e um brinquedo:

**Responder sucesso a tudo.** A sonda não tenta acertar o comportamento; ela tenta fazer o jogo
**andar**, porque é andando que ele mostra o que espera. Um `EFAILED` honesto pararia a
investigação na primeira chamada.

**Entregar outra sonda no ponteiro de saída.** Um método que devolve objeto escreve o ponteiro
num argumento, e responder sucesso sem escrever nada faz o jogo seguir com lixo e morrer no
primeiro uso — foi exatamente o que o Z-Wheel fez. Com a sonda entregando sondas, a observação
alcança a **família inteira**: o gerenciador devolve um banco, o banco devolve o que vier.

Só escreve onde é seguro: endereço alinhado, dentro da memória do jogo e **valendo zero**. Um
argumento que já aponta para alguma coisa não é destino de saída, e escrever nele estragaria
dado do jogo.

**Ler o texto dos argumentos.** É o que transforma `slot[3] (0x5373c, 0x10002f28)` em
`slot[3] ("tt_prefs.db", saída)`. Sem isso seria preciso descobrir onde o módulo foi mapeado
para ir buscar a string no arquivo — e a primeira tentativa de fazer isso deu uma base errada e
uma string sem sentido.

## O caso que a motivou

O Z-Wheel — que não é um jogo, é o aplicativo de loja do console — pedia `0x0102c4e8` e recusava
sair do lugar. O log dele já dizia o nome: `No SQLMGR: 20`, vinte e uma mil vezes, até estourar
a pilha.

Uma execução com a sonda deu a interface inteira:

```
classe 0x0102c4e8, objeto 0x30000050:
  slot[ 3] ("tt_prefs.db", 0x10002f28, 0x0)
classe 0x0102c4e8, objeto 0x30000090:
  slot[ 3] ("PRAGMA integrity_check", 0x94ce8, 0x10002f20)
  slot[ 1] (…)
```

Lê-se de cima para baixo: o gerenciador abre um banco pelo nome e devolve um objeto; o objeto
recebe uma instrução SQL, um ponteiro para dentro da faixa de código do módulo — um callback — e
um contexto. É a forma do `sqlite3_exec`, e foi o suficiente para implementar
[o `ISQLMgr`](../../src/sql.rs) sem nenhum header.

## A contagem é metade da informação

Cada linha do registro traz quantas vezes aquele slot foi chamado, e esse número separa duas
coisas muito diferentes: "o app chamou isto" e "o app está **preso** nisto".

Foi ela que fechou o diagnóstico da interface gráfica da Z-Wheel. Com cinco classes sondadas o
app passa do "Could not create root form" e cria catorze objetos — parece progresso. A contagem
mostra o que é de verdade:

```
classe 0x0100104f, objeto 0x30000510:
  slot[ 7] (…)   (14590410x)
  slot[ 4] (…)   (14590409x)
```

Dois métodos alternando quinze milhões de vezes: o app está num laço esperando uma resposta que
a sonda não sabe dar. Sondar não o fez andar, fez ele girar — e sem o contador isso passaria por
avanço.

## Combinar a resposta de um slot

`--sonda-resposta=0xCLSID:SLOT=VALOR` faz um slot responder o que se mandar, em vez de sucesso.
Existe porque responder sucesso a tudo tem um custo simétrico ao benefício: o jogo anda mais,
mas nunca chega ao fim de um laço em que ele espera ouvir "acabou".

Foi o que separou hipótese de fato na interface da Z-Wheel. O laço de quinze milhões de voltas
alterna dois métodos da classe `0x0100104f`; com o slot 4 respondendo qualquer coisa diferente
de zero, o laço **acaba** e o app segue — o que prova que aquele slot é a condição, e não o
outro. É informação que nenhuma quantidade de observação passiva daria.

## O pool de literais também identifica

A sonda diz o **formato** de um método; quem costuma dizer o **propósito** é o binário. Um módulo
ARM guarda as constantes de cada função num pool ao lado do código dela, e o `dbgprintf` dos
jogos deixa o nome do arquivo e a mensagem de erro no mesmo lugar. Então a vizinhança de uma
constante é, na prática, o nome dela.

Para usar isso é preciso calibrar onde o módulo foi mapeado. O jeito barato: pegar um ponteiro
de string que apareceu num rastro — o `IFILEMGR_OpenFile` da Z-Wheel recebeu `0x7afec` —,
procurar o conteúdo no arquivo e subtrair. No `tectoy.mod` deu base `0xbc80`, e a partir daí
qualquer endereço vira posição no arquivo.

Foi assim que as três classes da interface da Z-Wheel foram identificadas sem header nenhum:

| Classe | Onde a constante aparece | O que é |
|---|---|---|
| `0x01001011` | o erro que o app imprime ao recusá-la | o formulário raiz |
| `0x0100104f` | 20 bytes antes de `Could not create root form`, e também em "app history instance" e `Tectoy_LaunchMainMenu` | uma coleção genérica — usada em três lugares que não têm nada em comum além de guardar itens |
| `0x01035156` | entre `Unable to create instance of TrueType TYPEFACE` e `TrueType Dictionary` | a fonte TrueType |

## Onde a sonda acaba

Ela leva o jogo até o ponto em que ele **usa** o objeto de verdade, e para ali. Na Z-Wheel esse
ponto é uma instrução `blx r0` com `r0` valendo zero: o app leu um **campo** do objeto que
recebeu e chamou o que estava lá. Uma sonda é só uma vtable — não tem campos, e não há resposta
combinada que resolva isso.

É o sinal de que a descoberta terminou e a implementação começa. Saber disso evita o erro de
continuar sondando quando o que falta já não é informação.

## O erro de leitura que custou mais caro

Vale registrar porque não foi erro de emulação, foi de **método**.

A base de mapeamento do módulo eu calibrei procurando uma string no arquivo e subtraindo. Achei a
ocorrência errada, e desmontei tudo 0x3380 bytes deslocado — o que dá um código plausível: uma
sequência de instruções válidas, com desvios e chamadas, contando uma história inteiramente
falsa. Persegui essa história por várias execuções.

O que denunciou foi a discordância entre o desmontado e o `--code` do emulador: o rastro mostrava
o `lr` virando `0x78af0` numa instrução onde o desmontado não tinha chamada nenhuma. **Quando a
leitura estática e a execução discordam, quem está errado é a leitura.**

A base não precisava de calibração nenhuma: o carregador mapeia o arquivo inteiro em
`MODULE_BASE - MODULE_PREFIX` e preenche o prefixo com zeros, então o byte zero do arquivo é o
endereço `0x10000`. Estava em `loader.rs` o tempo todo.

## O que ela não faz

A sonda não diz **o nome** do método, só o número do slot e o formato. Quem dá nome é o uso: uma
string `"tt_prefs.db"` num slot que devolve objeto é `OpenDatabase`, e é assim que o nome entra
na tabela de slots — com o comentário dizendo que a origem foi observação, não documentação.

E ela mente por construção: responde sucesso a métodos que no console poderiam falhar. Serve
para descobrir, não para rodar. Nenhuma execução normal deve depender dela.
