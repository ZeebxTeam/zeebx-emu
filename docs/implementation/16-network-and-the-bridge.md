# 16 — Network, cryptography and the bridge

This document covers what the emulator does when a game wants to talk to the internet. It exists
because of Zeeboids, which is the only title in the library whose content **depends** on a
server: without a network it opens, and does none of what it was made to do.

## A deliberately small HTTP client

`net.rs`. Zeebo games speak **plain HTTP** — Zeeboids points to `http://www.zeeboids.com/...` and
there is no HTTPS anywhere in the module. A client library would bring TLS, redirects,
`keep-alive` and policies that nobody here uses; what is needed is a POST with a binary body and
the response back.

Three decisions that look like details and are not:

- **Only `http://`.** An `https://` is a request we do not know how to serve, and answering as if
  we did would be worse than refusing.
- **Five seconds of waiting.** It is short for a real server, and that is what we want: the
  emulator cannot stop because the other side does not answer. The game already has its own
  timeout, and failing fast hands control back to it.
- **A one-megabyte cap per response.** What travels here are text lists separated by `;`.

Giving network access to a binary of outside origin is a project decision, and that is why it
**is not silent**: every request goes into the report, and the `Machine` can be created without it
(`--no-network`).

### Redirecting the server

`--server=HOST[:PORT]`, or the `ZEEBX_SERVER` variable. `www.zeeboids.com` went offline with
TecToy; without a redirect, studying the protocol would require tampering with the game binary or
with the machine's DNS. The variable exists because whoever runs the emulator through the
interface does not pass command-line arguments.

The redirect changes **where** the connection goes, and nothing else: the path, the headers and
the body stay as the game built them.

## The cryptography

`brew/crypto.rs` carries AES-128 in CBC, which is what BREW provides through
`AEECLSID_BlockAES128`, and `IHash` (MD5). It is written here instead of coming from a dependency
because it is small, closed and verifiable: the tests use the vectors from the specifications
themselves — FIPS-197 for the block and NIST SP 800-38A for the chaining.

The point of method: **what the game encrypts, the report records in the clear**. Whoever studies
a protocol needs the text before the cipher, and it passes through us anyway. Without that, the
Zeeboids request would reach the report as an opaque block and the protocol would have to be read
in the binary line by line.

The `IHash` table was wrong from slot 2 onward — the names came from a header of another BREW
version. An `Update` called in place of `Final` does not error out: it returns a digest that is
consistent with itself and different from what the server expects.

## The bridge

`bridge.rs`. **This is not emulation**, and that is why it lives apart, is optional and carries
the name of what it is. Everything else in Zeebx implements BREW API or reads the console's vtables
— things that hold for any title. What is in the bridge holds for **one** game.

### Why it exists

To study a protocol, the game has to **consume** the response, not just receive it. In Zeeboids
the response becomes a vector of strings inside an object of the game itself, and those strings
have to come out of its allocator: handing over outside memory makes the manager complain when it
goes to free them (`TTDMemoryManager.cpp:916`). On the console, what filled that vector was the
firmware, which naturally used the right allocator.

The bridge keeps two addresses of the module: the `malloc` of the game's manager, with the
observed signature `(size, 0, line, file, 1)`, and the game's own **response parser**. Using its
parser is better than reproducing it: there is no format to get right and no state flag to guess.

### The rule the bridge cannot break

It **does not touch what travels**. It does not change the request, it does not change the
response, it does not exist on the server side. A real Zeebo talking to the same server sees
exactly the same HTTP.

The corollary matters as much as the rule: **the bridge never justifies changing the server.** If
we tweaked the response until our way of delivering it accepted it, we would get it right here and
wrong on the console. The format is decided by what the game does, and the final proof is the real
device.

### What fell along the way

The bridge once brought the game down — invalid access at `0x654ac`, inside the memory manager.
Two hypotheses fell before the cause, and they are recorded so nobody redoes them:

- **It was not the element format.** I came to think the vector held `ttdString` objects —
  `{length, pointer}`, as the constructor at `0xa85e0` builds them. It does not: the handler calls
  `atoi` directly on the element, so those really are `char *`.
- **It was not the call.** The allocator receives pool index 0, which it requires to be below 32,
  and reads it in the fifth argument, at `[sp+0x30]`.

It was **the timing**. The response waits in a queue and is deposited at the boundary between two
API calls — the same one the signals and callbacks use, when the guest is not inside anything.
Calling the allocator from inside the dispatch re-entered a manager that was in the middle of an
operation.

It is the same pattern that shows up in [04-time](04-time.md) and in
[14-z-wheel](14-z-wheel-and-efs2.md): almost every "the game broke in a way that makes no sense"
problem turned out to be *when* we call, not *what* we call.

### An empty body is a response

A `Content-Length: 0` is not a failure: it is the server saying "nothing to sync". The game needs
the end-of-stream notice anyway, and without it it keeps waiting. And the end of stream comes
**after** the body is delivered — invert the order and the game skips the consumption.

### `SourceFromMemory` is also what its name says

The bridge comes in through `ISourceUtil::SourceFromMemory` because that is where the Zeeboids POST
body exists whole and in the clear. But the method has a contract of its own, and it holds for
everyone: wrapping a piece of memory in an `ISource`.

Treating **every** call as a send, whoever was not sending got `EFAILED`. Prey Evil wraps 715 KB of
data from its own package here, with no network at all: it was left without the source and drew a
black screen per frame, 717 times, while reading zero bytes. Today the search for the URL decides:
found, it is a send and the bridge goes on as before; not found, the piece of memory becomes an
`ISource`. After the change it creates 248 sources and reads from them — it still does not draw,
which is another matter.

## The study server

It lives in `zeeboids-server/`, outside this repository and with its own git, because it is not
part of the emulator. The protocol worked out from the binary is in
`zeeboids-server/docs/protocolo.md`, in that other repository — that is where everything
documented about Zeeboids' communication goes.

An error worth recording because it was not the emulator's: the import response has to carry
`lid;id;zid`. Without all three, the game read the IMEI as if it were the `lid`, created a
corrupted Zeeboid with id 0 and F.C. crashed at `0x000ca6fc`. The symptom showed up in one game
and the cause was in another — in the first sync done by Zeeboids.
