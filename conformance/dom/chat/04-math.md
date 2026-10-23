# Bayes' theorem, worked through

The probability of a hypothesis $$H$$ given evidence $$E$$ is

$$
P(H \mid E) = \frac{P(E \mid H)\,P(H)}{P(E)}
$$

where the denominator expands by the law of total probability:

$$
P(E) = \sum_{i} P(E \mid H_i)\,P(H_i)
$$

## Example: a medical test

- Prevalence: $$P(D) = 0.01$$
- Sensitivity: $$P(+ \mid D) = 0.95$$
- False-positive rate: $$P(+ \mid \neg D) = 0.05$$

Plugging in:

$$
P(D \mid +) = \frac{0.95 \times 0.01}{0.95 \times 0.01 + 0.05 \times 0.99} \approx 0.161
$$

So even with a positive result, there's only about a **16%** chance of disease[^1]. This surprises most people — it's the *base-rate fallacy*[^2].

| Prevalence | $$P(D \mid +)$$ |
|---|---|
| 0.1% | 1.9% |
| 1% | 16.1% |
| 10% | 67.9% |

[^1]: Assuming the test results are conditionally independent of everything else.
[^2]: See Kahneman & Tversky, *Judgment under Uncertainty* (1974).
