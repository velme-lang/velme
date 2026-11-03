# Loan comparison

Source: DMN TCK `compliance-level-3/0014-loan-comparison`, restated in plain English. Both arms of the benchmark get
this text and the examples in `examples.json`; neither sees the DMN model, the oracle or the held-out inputs.

## Input

- **Requested amount:** the amount the borrower asks for, in exact cents (at most two decimal places), from 5000.00
  to 1500000.00.

## Lender table

Ten loan products, always in this order. Rate is the annual interest rate as a fraction (`0.03500` is 3.5%); points
are a percentage of the requested amount; fee is an amount of money.

| # | Lender name | Rate | Points | Fee |
|---|---|---|---|---|
| 1 | `"Oceans Capital"` | 0.03500 | 0 | 0 |
| 2 | `"eClick Lending"` | 0.03200 | 1.1 | 2700 |
| 3 | `"eClickLending"` | 0.03375 | 0.1 | 1200 |
| 4 | `"AimLoan"` | 0.03000 | 1.1 | 3966 |
| 5 | `"Home Loans Today"` | 0.03125 | 1.1 | 285 |
| 6 | `"Sebonic"` | 0.03125 | 0.1 | 4028 |
| 7 | `"AimLoan"` | 0.03125 | 0.1 | 4317 |
| 8 | `"eRates Mortgage"` | 0.03125 | 1.1 | 2518 |
| 9 | `"Home Loans Today"` | 0.03250 | 0.1 | 822 |
| 10 | `"AimLoan"` | 0.03250 | 0 | 1995 |

Lender names repeat; a product is identified by its whole row, not its name.

A product passed to any function is a row of this table. The same rules must hold for any product whose rate is
above 0 and at most 1, whose points are from 0 to 100 and whose fee is from 0 to 100000.

## Output

One ranked-products record with four lists. Every list holds one metric record per product (ten in all).

**Metric record** for a product, given the requested amount `A`, with `r` its rate:

- lender name, rate, points and fee: copied from the product.
- **Loan amount** `L` = `A × (1 + points / 100) + fee`.
- **Down payment amount** = `0.2 × L`.
- **Payment amount** (monthly, over 360 months) = `L × r / 12 / (1 − (1 + r / 12)^(−360))`.
- **Equity 36-month percentage** = `1 − (B / A) × 0.8`, where `B` is the balance after 36 months:
  `B = L × (1 + r / 12)^36 − P × ((1 + r / 12)^36 − 1) / r`, with `P` the payment amount. (The last division is by
  the annual rate `r`, not by `r / 12`.)

**Metrics table** for a list of products: one metric record per product, in the list's order.

**Ranking** a metrics table gives three more lists of the same records:

- **Rank by down payment:** lowest down payment amount first.
- **Rank by monthly payment:** lowest payment amount first.
- **Rank by equity percentage:** highest equity 36-month percentage first.

When two records are equal on the value a list ranks by, they keep their order in the metrics table (the one
earlier in the table comes first), in all three ranked lists. Ranking works on any list of metric records, whatever
their values.

**The ranked-products record** for a requested amount: the metrics table of the whole lender table, in lender-table
order, and its three rankings.

All arithmetic is exact decimal. Payment amount and equity percentage do not terminate; they are compared to 8
decimal places, and the examples show them rounded to 8 places. Every other number, and the order of every list, is
compared exactly.
