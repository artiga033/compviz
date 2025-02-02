# compviz

**compviz** aims to visualize the btrfs filesystem compression statistics.

## Features
As of now, it is simply a rewrite of [compsize](https://github.com/kilobyte/compsize) in Rust.

However, compviz takes advantage of the easy parallelism that Rust provides, so it should be faster if you have multiple CPU cores. Check the benchmarks on my `13th Gen Intel(R) Core(TM) i5-13490F`:

| Command               | Mean [ms]    |   Min [ms] |   Max [ms] | Ralative      | x Times Faster   |
|:----------------------|:-------------|-----------:|-----------:|:--------------|:-----------------|
| `compsize`            | 883.93±8.18  |    875.032 |    904.783 | 100.00%       | 1.00             |
| `compviz(1 thread)`   | 943.41±3.57  |    938.748 |    948.599 | 106.73%±1.07% | 0.94±0.01        |
| `compviz(2 threads)`  | 497.40±3.39  |    493.394 |    504.788 | 56.27%±0.65%  | 1.78±0.02        |
| `compviz(3 threads)`  | 359.16±6.07  |    351.615 |    373.391 | 40.63%±0.78%  | 2.46±0.05        |
| `compviz(4 threads)`  | 287.48±1.90  |    284.497 |    290.025 | 32.52%±0.37%  | 3.07±0.03        |
| `compviz(5 threads)`  | 241.02±6.07  |    236.355 |    257.283 | 27.27%±0.73%  | 3.67±0.10        |
| `compviz(6 threads)`  | 207.48±2.63  |    202.456 |    212.877 | 23.47%±0.37%  | 4.26±0.07        |
| `compviz(7 threads)`  | 191.60±2.32  |    188.854 |    196.265 | 21.68%±0.33%  | 4.61±0.07        |
| `compviz(8 threads)`  | 178.03±1.35  |    175.852 |    180.732 | 20.14%±0.24%  | 4.97±0.06        |
| `compviz(9 threads)`  | 168.95±2.58  |    165.517 |    175.895 | 19.11%±0.34%  | 5.23±0.09        |
| `compviz(10 threads)` | 168.96±8.01  |    160.387 |    189.383 | 19.11%±0.92%  | 5.23±0.25        |
| `compviz(11 threads)` | 178.69±20.74 |    156.699 |    221.654 | 20.21%±2.35%  | 4.95±0.58        |
| `compviz(12 threads)` | 179.70±22.52 |    155.721 |    219.595 | 20.33%±2.56%  | 4.92±0.62        |
| `compviz(13 threads)` | 184.90±15.12 |    150.380 |    205.035 | 20.92%±1.72%  | 4.78±0.39        |
| `compviz(14 threads)` | 192.46±22.12 |    153.112 |    216.941 | 21.77%±2.51%  | 4.59±0.53        |
| `compviz(15 threads)` | 185.12±17.19 |    162.086 |    216.368 | 20.94%±1.95%  | 4.77±0.45        |
| `compviz(16 threads)` | 192.81±18.81 |    163.218 |    216.273 | 21.81%±2.14%  | 4.58±0.45        |

## Parallelism

According to the result of benchmarks, by default, compviz uses as many threads as:

- `RAYON_NUM_THREADS` env var if set
- based of the number of logical CPUs, let which be `n`:
  - `n` if `n <= 6`
  - `n/2` if `6 < n < 24`
  - `24` if `n >= 24`