# Secure Satellite Collision Avoidance

Two-party semi-honest secure computation of a fixed-point arithmetic circuit,
with a trusted dealer supplying the correlated randomness of the offline phase.
The protocol follows Hemenway, Lu, Ostrovsky and Welser, *High-precision Secure
Computation of Satellite Collision Probabilities* (SCN 2016): values live as
additive shares in the ring `Z_{2^n}`, inputs are shared by their owner,
constants are held by one party, additions are local, and multiplications
consume one Beaver triple each.

The two parties talk over a TCP socket and are meant to be started in separate
terminals — there is no in-process simulation.

## Number representation

Every wire carries a two's-complement fixed-point number: the real `x` is
represented by `round(x * 2^L_BIT) mod 2^N_BIT`. The widths are set in `.env`:

| setting | meaning |
| --- | --- |
| `N_BIT` | total width; the ring is `Z_{2^N_BIT}` (2..=64) |
| `I_BIT` | integer bits, sign bit included |
| `L_BIT` | fractional bits |

`I_BIT + L_BIT` must equal `N_BIT`. Any setting in `.env` can be overridden for
a single run by exporting the same name as an environment variable.

## Running it

```sh
cargo run -- info        # what the parties are about to evaluate
cargo run -- dealer      # offline phase: writes preprocessing/party{0,1}.bin
```

Then, in two terminals:

```sh
cargo run -- party 0     # listens on PARTY0_ADDR
cargo run -- party 1     # connects to it
```

Party 0 must be able to bind `PARTY0_ADDR`; party 1 retries the connection for
ten seconds, so the start order does not matter much.

Each party reads the cleartext values of the input wires it owns from
`inputs/party<id>.txt` and its half of the correlated randomness from
`preprocessing/party<id>.bin`. Neither file is ever shared with the peer. The
preprocessing carries a fingerprint of the netlist and of the number
representation, so a party refuses material generated for a different circuit
rather than producing a silently wrong answer.

### End-to-end check

`circuit/smoke.netlist` exercises everything that is wired up so far. Products
are not rescaled yet (see below), so run it with `L_BIT=0`, which makes the
wires hold plain integers:

```sh
export CIRCUIT=circuit/smoke.netlist N_BIT=64 I_BIT=64 L_BIT=0
cargo run -- dealer
cargo run -- party 0     # in one terminal
cargo run -- party 1     # in another
```

With the inputs shipped in `inputs/`, both parties print `O1 = -3`, which is
`(7 + -5) * -5 + 7`.

## Cost accounting

Both phases report what they cost. The dealer prints its own summary:

```text
offline phase
  beaver triples   2000
  generation       2.433 ms
  storage          523.479 us
  wall clock       2.956 ms
  written          preprocessing/party0.bin (46.91 KiB)
  written          preprocessing/party1.bin (46.91 KiB)
  peak memory      3.61 MiB
```

and each party prints its own after the outputs:

```text
online phase (party 1)
  loading          8.418 ms
  connecting       53.699 us (includes waiting for the peer)
  protocol         4.265 ms
  wall clock       12.737 ms
  rounds           3
  sent             31.27 KiB
  received         31.27 KiB
  peak memory      3.86 MiB
```

`protocol` is the number to compare against the paper: it covers input sharing,
circuit evaluation and output reconstruction, and excludes reading files and
establishing the socket. `connecting` is dominated by how long party 0 idles
before party 1 is started, so it says more about the operator than about the
protocol. `rounds` counts messages in each direction, including the input and
output rounds — the example above is 2000 multiplications batched into a single
round, plus one round each for inputs and outputs.

`peak memory` is the process high-water mark (`VmHWM`), so it covers the
circuit, the preprocessing material and the Rust runtime together; it is only
available on Linux and prints as unavailable elsewhere. The `written` lines give
the storage cost of the offline phase, which is the figure that matters when the
preprocessing has to be shipped ahead of time.

## The netlist

One gate per line, whitespace separated. Operands refer to gates defined on an
earlier line, so the file is already in topological order. `#` starts a comment.

```text
I1              input wire, owner defaults to party 0, then 1, then 0, ...
I2 P1           input wire with an explicit owner
G2 C 40         public constant
G4 + I2 I1      addition, evaluated locally
G1 * I1 I2      multiplication, consumes one Beaver triple
G3 >> c G1 G2   truncate G1 by the constant on wire G2
G7 < 0 G6       the bit 1{G6 < 0}
O1 G7           reveal a wire to both parties
```

A constant literal is stored unscaled. Whether it means a real number or a plain
bit count depends on the gate that reads it: an addition or a multiplication
encodes it as fixed point, while a truncation takes it as a shift amount.

The circuit is evaluated in layers. All interactive gates that sit in the same
layer are opened in a single message, so the cost is one round per layer of
multiplications rather than one round per multiplication. `cargo run -- info`
prints the schedule.

## Status

Implemented: input, output, constant, addition, multiplication (Beaver triples),
and the surrounding machinery — netlist parsing, the round schedule, the dealer,
per-party preprocessing files, and the socket transport.

Not implemented yet: the truncation gate (`>>`) and the less-than-zero gate
(`<`). Both are parsed, appear in the round schedule, and are counted in the
circuit summary; evaluating one currently fails with an explicit error. Until
truncation exists, a product of two fixed-point values keeps `2 * L_BIT`
fractional bits and cannot be combined with unscaled values — which is why the
smoke test runs at `L_BIT=0`.

`libfss/` is the function-secret-sharing library that the two remaining gates
will be built on. Nothing in the protocol depends on it yet.
