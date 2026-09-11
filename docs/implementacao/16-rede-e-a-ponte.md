# 16 — Rede, criptografia e a ponte

Este documento cobre o que o emulador faz quando um jogo quer falar com a internet. Ele existe
por causa do Zeeboids, que é o único título da biblioteca cujo conteúdo **depende** de um
servidor: sem rede ele abre, e não faz nada do que foi feito para fazer.

## Um cliente HTTP pequeno de propósito

`rede.rs`. Os jogos do Zeebo falam **HTTP puro** — o Zeeboids aponta para
`http://www.zeeboids.com/...` e não há HTTPS em lugar nenhum do módulo. Uma biblioteca de
cliente traria TLS, redirecionamento, `keep-alive` e políticas que ninguém aqui usa; o que se
precisa é de um POST com corpo binário e a resposta de volta.

Três decisões que parecem detalhe e não são:

- **Só `http://`.** Um `https://` é um pedido que não sabemos atender, e responder como se
  soubéssemos seria pior do que recusar.
- **Cinco segundos de espera.** É curto para um servidor de verdade, e é o que se quer: o
  emulador não pode parar porque o outro lado não responde. O jogo já tem o próprio tempo
  limite, e falhar rápido devolve a ele o controle.
- **Um mega de teto por resposta.** O que trafega aqui são listas de texto separadas por `;`.

Dar rede a um binário de origem externa é decisão de projeto, e por isso ela **não é
silenciosa**: cada requisição entra no relatório, e a `Machine` pode ser criada sem ela
(`--sem-rede`).

### Desviar o servidor

`--servidor=MAQUINA[:PORTA]`, ou a variável `ZEEBX_SERVIDOR`. O `www.zeeboids.com` saiu do ar
com a TecToy; sem um desvio, estudar o protocolo exigiria mexer no binário do jogo ou no DNS da
máquina. A variável existe porque quem roda o emulador pela interface não passa argumentos de
linha de comando.

O desvio troca **para onde** a conexão vai, e nada mais: o caminho, os cabeçalhos e o corpo
seguem como o jogo os montou.

## A criptografia

`brew/crypto.rs` traz o AES-128 em CBC, que é o que o BREW entrega pelo `AEECLSID_BlockAES128`, e o
`IHash` (MD5). Está escrito aqui em vez de vir de uma dependência porque é pequeno, fechado e
verificável: os testes usam os vetores das próprias especificações — o FIPS-197 para o bloco e o
NIST SP 800-38A para o encadeamento.

O ponto de método: **o que o jogo cifra, o relatório registra em claro**. Quem estuda um
protocolo precisa do texto antes da cifra, e ele passa por nós de qualquer forma. Sem isso, a
requisição do Zeeboids chegaria ao relatório como um bloco opaco e o protocolo teria de ser
lido no binário linha a linha.

A tabela do `IHash` esteve errada do slot 2 em diante — os nomes vinham de um header de outra
versão do BREW. Um `Update` chamado no lugar do `Final` não dá erro: devolve um resumo que é
consistente consigo mesmo e diferente do que o servidor espera.

## A ponte

`ponte.rs`. **Isto não é emulação**, e por isso mora separado, é opcional e tem o nome do que é.
Todo o resto do Zeebx implementa API do BREW ou lê vtable do console — coisas que valem para
qualquer título. O que está na ponte vale para **um** jogo.

### Por que existe

Para estudar um protocolo é preciso que o jogo **consuma** a resposta, e não só que ela chegue.
No Zeeboids a resposta vira um vetor de strings dentro de um objeto do próprio jogo, e essas
strings têm de sair do alocador dele: entregar memória de fora faz o gerenciador reclamar quando
vai liberá-las (`TTDMemoryManager.cpp:916`). No console quem preenchia esse vetor era o
firmware, que naturalmente usava o alocador certo.

A ponte guarda dois endereços do módulo: o `malloc` do gerenciador do jogo, com a assinatura
observada `(tamanho, 0, linha, arquivo, 1)`, e o **parser de resposta** do próprio jogo. Usar o
parser dele é melhor do que reproduzi-lo: não há formato para acertar nem marca de estado para
adivinhar.

### A regra que a ponte não pode quebrar

Ela **não toca no que trafega**. Não muda a requisição, não muda a resposta, não existe do lado
do servidor. Um Zeebo de verdade falando com o mesmo servidor vê exatamente o mesmo HTTP.

O corolário importa tanto quanto a regra: **a ponte nunca justifica mudar o servidor.** Se
ajustássemos a resposta até o nosso jeito de entregar aceitá-la, acertaríamos aqui e erraríamos
no console. O formato se decide pelo que o jogo faz, e a prova final é o aparelho de verdade.

### O que caiu no caminho

A ponte chegou a derrubar o jogo — acesso inválido em `0x654ac`, dentro do gerenciador de
memória. Duas hipóteses caíram antes da causa, e ficam registradas para ninguém refazê-las:

- **Não era o formato do elemento.** Cheguei a achar que o vetor guardava objetos `ttdString` —
  `{comprimento, ponteiro}`, como o construtor em `0xa85e0` monta. Não é: o tratador chama
  `atoi` direto no elemento, então ali são `char *` mesmo.
- **Não era a chamada.** O alocador recebe o índice de pool 0, que ele exige menor que 32, e o
  lê no quinto argumento, em `[sp+0x30]`.

Era **o momento**. A resposta espera numa fila e é depositada na fronteira entre duas chamadas
de API — a mesma que os sinais e os callbacks usam, quando o guest não está dentro de nada.
Chamar o alocador de dentro do despacho reentrava num gerenciador que estava no meio de uma
operação.

É o mesmo padrão que aparece no [04-tempo](04-tempo.md) e no
[14-z-wheel](14-z-wheel-e-o-efs2.md): quase todo problema de "o jogo quebrou de um jeito que não
faz sentido" acabou sendo *quando* chamamos, não *o que* chamamos.

### Corpo vazio é resposta

Um `Content-Length: 0` não é falha: é o servidor dizendo "nada a sincronizar". O jogo precisa do
aviso de fim de fluxo mesmo assim, e sem ele fica esperando. E o fim de fluxo vem **depois** da
entrega do corpo — invertendo a ordem, o jogo pula o consumo.

## O servidor de estudo

Fica em `zeeboids-server/`, fora deste repositório e com git próprio, porque não é parte do
emulador. O protocolo levantado do binário está em
[`zeeboids-server/docs/protocolo.md`](../../zeeboids-server/docs/protocolo.md) — é lá que vai
tudo o que se documenta sobre a comunicação do Zeeboids.

Um erro que vale registrar porque não foi do emulador: a resposta de importação precisa trazer
`lid;id;zid`. Sem os três, o jogo lia o IMEI como se fosse o `lid`, criava um Zeeboid de id 0
corrompido e o F.C. quebrava em `0x000ca6fc`. O sintoma aparecia num jogo e a causa estava em
outro — na primeira sincronização feita pelo Zeeboids.
