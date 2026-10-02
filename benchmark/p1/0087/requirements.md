# Loan origination decision

Source: DMN TCK `compliance-level-3/0087-chapter-11-example` (the DMN 1.x specification's chapter 11 example),
restated in plain English. Both arms of the benchmark get this text and the examples in `examples.json`; neither
sees the DMN model, the oracle or the held-out inputs.

## Inputs

- **Applicant:** age (whole years, 18 to 120), marital status (`"S"` or `"M"`), employment status (`"EMPLOYED"`,
  `"SELF-EMPLOYED"`, `"STUDENT"` or `"UNEMPLOYED"`), whether they are an existing customer, and their monthly income,
  monthly repayments and monthly expenses.
- **Requested product:** product type (`"STANDARD LOAN"` or `"SPECIAL LOAN"`), annual interest rate as a fraction
  (`0.08` is 8%), term in months (a whole number, at least 1) and amount.
- **Bureau data:** whether the applicant is bankrupt, and their credit score (a whole number from 0 to 999).

## Output

One decision record with ten fields, computed as below. All arithmetic is exact decimal; nothing is rounded.

1. **Application risk score:** the sum of three point values.
   - Age: 18 up to but not including 22 → 32; 22 up to but not including 26 → 35; 26 up to but not
     including 36 → 40; 36 up to but not including 50 → 43; 50 or over → 48.
   - Marital status: `"S"` → 25; `"M"` → 45.
   - Employment status: `"UNEMPLOYED"` → 15; `"STUDENT"` → 18; `"EMPLOYED"` → 45; `"SELF-EMPLOYED"` → 36.
2. **Pre-bureau risk category,** from the existing-customer flag and the application risk score:
   - New customer: below 100 → `"HIGH"`; 100 up to but not including 120 → `"MEDIUM"`; 120 to 130 inclusive →
     `"LOW"`; above 130 → `"VERY LOW"`.
   - Existing customer: below 80 → `"DECLINE"`; 80 up to but not including 90 → `"HIGH"`; 90 to 110 inclusive →
     `"MEDIUM"`; above 110 → `"LOW"`.
3. **Required monthly installment:** the monthly repayment plus a monthly fee.
   - Monthly fee: 20.00 for `"STANDARD LOAN"`, 25.00 for `"SPECIAL LOAN"`.
   - Monthly repayment, with `r` = annual rate / 12 and `n` = term:
     `(amount × r) / (1 − (1 + r)^(−n))`.
4. **Pre-bureau affordability** (true or false), using the pre-bureau risk category:
   - Disposable income = monthly income − (monthly repayments + monthly expenses).
   - Credit contingency factor by risk category: `"HIGH"` or `"DECLINE"` → 0.6; `"MEDIUM"` → 0.7; `"LOW"` or
     `"VERY LOW"` → 0.8.
   - Affordable when disposable income × factor is strictly greater than the required monthly installment.
5. **Eligibility:** `"INELIGIBLE"` if the pre-bureau risk category is `"DECLINE"`, or pre-bureau affordability is
   false, or the applicant is under 18; otherwise `"ELIGIBLE"`.
6. **Bureau call type,** from the pre-bureau risk category: `"HIGH"` or `"MEDIUM"` → `"FULL"`; `"LOW"` → `"MINI"`;
   `"VERY LOW"` or `"DECLINE"` → `"NONE"`.
7. **Strategy:** `"DECLINE"` if ineligible; for an eligible applicant, `"BUREAU"` when the bureau call type is
   `"FULL"` or `"MINI"` and `"THROUGH"` when it is `"NONE"`.
8. **Post-bureau risk category,** from the existing-customer flag, the application risk score and the credit score:
   - New customer, score below 120: credit score below 590 → `"HIGH"`; 590 to 610 inclusive → `"MEDIUM"`; above
     610 → `"LOW"`.
   - New customer, score 120 to 130 inclusive: credit score below 600 → `"HIGH"`; 600 to 625 inclusive →
     `"MEDIUM"`; above 625 → `"LOW"`.
   - New customer, score above 130: `"VERY LOW"`.
   - Existing customer, score 100 or below: credit score below 580 → `"HIGH"`; 580 to 600 inclusive → `"MEDIUM"`;
     above 600 → `"LOW"`.
   - Existing customer, score above 100: credit score below 590 → `"HIGH"`; 590 to 615 inclusive → `"MEDIUM"`;
     above 615 → `"LOW"`.
9. **Post-bureau affordability:** as in 4, but using the post-bureau risk category.
10. **Routing:** `"DECLINE"` if post-bureau affordability is false or the applicant is bankrupt; otherwise
    `"REFER"` if the post-bureau risk category is `"HIGH"` or the credit score is below 580; otherwise `"ACCEPT"`.

Every field is computed for every applicant, even one whose strategy is `"DECLINE"`.
