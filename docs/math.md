# Math

A Pascal contract pays 1 USDC if its outcome happens, 0 if not, and trades at a price
$p \in (0, 1)$. The derivation is in the [PDF](hedging-paper.md).

## One event: perfect hedge

The house fills the user's stake $S$ on Pascal at an average price $f$ and holds the contracts.
They pay exactly what the user is owed, in every outcome. The odd is the inverse of the average
price paid, less the haircut $m$:

$$
o = \frac{1 - m}{f}
$$

## Several events: a hedging strategy

The payout is the product of the legs, $P = S \prod_i (1 - m_i)/f_i$, but Pascal has no
contract on "all legs win", so no portfolio pays $P$ exactly. The house needs a strategy:

| Strategy | Who covers the gap | Odd for the user | House variance |
| --- | --- | --- | --- |
| **Naive static** (today) | the house, any gap | product of prices | not controlled |
| **Static superhedge** | nobody: always covered | the least likely leg alone | none |
| **Dynamic hedge** | the house, a known amount | close to the product of prices | bounded |

- **Naive static.** At mint, hold $\Delta_i = P \prod_{j \ne i} f_j$ contracts per leg, re-trade
  only when a leg settles, and let the house pay whatever the portfolio misses. Not shippable:
  nothing bounds the house's losses.
- **Static superhedge.** Cover $P$ in every outcome. It costs $P \min_i p_i$ (the Fréchet bound),
  so the odd is $(1 - m)/\min_i p_i$, and two single bets would pay more.
- **Dynamic hedge.** Hold $\Delta_i(t) = P \prod_{j \ne i} v_j(t)$ at the legs' live fair values
  and re-trade as they move. The expected cost is known, the variance is bounded, and the odd
  lies between the superhedge's and $(1 - m)/\prod_i p_i$.

Whatever the strategy, the user is always paid: the vault locks $P$ at mint.
