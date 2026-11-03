# Revenue

## Odds haircut

3% per leg (5% when legs overlap), taken inside the total:

| Legs | In sequence | Overlapping |
| --- | --- | --- |
| 1 | 3.0% | 5.0% |
| 2 | 5.9% | 9.8% |
| 3 | 8.7% | 14.3% |
| 5 | 14.1% | 22.6% |

Cash-out takes 2% of the ticket's value.

Example: 10 USDC on two overlapping legs pays 53.80. Expected take: 0.975 USDC, before about
0.20 USDC of Pascal fees.

## Copy fee

Copying a tip costs 2% of the stake, on top. The program splits it at mint: 80% to the tipster,
20% to the house. A copy also pays the normal haircut.
