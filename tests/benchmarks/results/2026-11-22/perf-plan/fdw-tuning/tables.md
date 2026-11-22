<!-- SPDX-License-Identifier: Apache-2.0 -->
# Reproduced FDW tuning tables

Five measured repetitions plus excluded value-check warmups per streaming/terminal cell. Cache cold/hot phases are intentional and separately labelled. Shared-host observations, not isolated production latency. Native HTTP never has a terminal-policy contract; its policy label only marks scheduling alongside that client configuration. The original streaming-full native codec was unmatched: use native-uncompressed as a separate control, not a paired client-overhead estimate.

## streaming-full

| workload | policy | block_size | route | median ms | min ms | max ms | server median ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | 1024 | direct | 214.020 | 189.295 | 357.814 | 177.000 |
| narrow | flush_first | 1024 | native | 240.069 | 210.603 | 270.759 | 237.000 |
| narrow | flush_first | 1024 | pg_backend | 316.518 | 285.333 | 480.377 | 176.000 |
| narrow | flush_first | 1024 | pg_copy | 550.834 | 487.237 | 7853.643 | 233.000 |
| narrow | flush_first | 4096 | direct | 93.424 | 84.427 | 151.491 | 57.000 |
| narrow | flush_first | 4096 | native | 94.357 | 79.266 | 113.119 | 93.000 |
| narrow | flush_first | 4096 | pg_backend | 274.697 | 226.294 | 289.146 | 155.000 |
| narrow | flush_first | 4096 | pg_copy | 462.507 | 407.163 | 791.095 | 236.000 |
| narrow | flush_first | 16384 | direct | 71.074 | 62.804 | 132.416 | 30.000 |
| narrow | flush_first | 16384 | native | 48.342 | 44.442 | 68.400 | 45.000 |
| narrow | flush_first | 16384 | pg_backend | 278.074 | 229.964 | 346.617 | 165.000 |
| narrow | flush_first | 16384 | pg_copy | 503.067 | 461.481 | 603.246 | 282.000 |
| narrow | in_band | 1024 | direct | 288.329 | 254.003 | 318.779 | 286.000 |
| narrow | in_band | 1024 | native | 229.929 | 182.805 | 243.394 | 226.000 |
| narrow | in_band | 1024 | pg_backend | 313.873 | 273.616 | 353.729 | 297.000 |
| narrow | in_band | 1024 | pg_copy | 484.904 | 456.957 | 644.543 | 307.000 |
| narrow | in_band | 4096 | direct | 168.065 | 160.745 | 179.075 | 165.000 |
| narrow | in_band | 4096 | native | 84.237 | 75.432 | 95.969 | 81.000 |
| narrow | in_band | 4096 | pg_backend | 246.137 | 220.896 | 278.157 | 182.000 |
| narrow | in_band | 4096 | pg_copy | 448.927 | 407.324 | 579.461 | 260.000 |
| narrow | in_band | 16384 | direct | 143.669 | 131.251 | 192.208 | 141.000 |
| narrow | in_band | 16384 | native | 51.105 | 45.365 | 69.965 | 48.000 |
| narrow | in_band | 16384 | pg_backend | 278.107 | 193.359 | 291.541 | 196.000 |
| narrow | in_band | 16384 | pg_copy | 541.124 | 457.891 | 641.971 | 280.000 |
| smalltext | flush_first | 1024 | direct | 372.755 | 297.470 | 558.011 | 316.000 |
| smalltext | flush_first | 1024 | native | 355.178 | 276.319 | 529.214 | 350.000 |
| smalltext | flush_first | 1024 | pg_backend | 677.334 | 496.138 | 3495.096 | 538.000 |
| smalltext | flush_first | 1024 | pg_copy | 1235.958 | 909.573 | 1449.028 | 1027.000 |
| smalltext | flush_first | 4096 | direct | 185.145 | 174.561 | 257.498 | 156.000 |
| smalltext | flush_first | 4096 | native | 197.886 | 175.465 | 281.749 | 193.000 |
| smalltext | flush_first | 4096 | pg_backend | 486.062 | 419.116 | 669.546 | 417.000 |
| smalltext | flush_first | 4096 | pg_copy | 1341.695 | 994.750 | 2577.008 | 1080.000 |
| smalltext | flush_first | 16384 | direct | 159.766 | 142.894 | 287.704 | 120.000 |
| smalltext | flush_first | 16384 | native | 126.089 | 116.037 | 243.466 | 120.000 |
| smalltext | flush_first | 16384 | pg_backend | 552.556 | 468.951 | 677.025 | 438.000 |
| smalltext | flush_first | 16384 | pg_copy | 1229.924 | 1004.128 | 1720.250 | 1017.000 |
| smalltext | in_band | 1024 | direct | 995.129 | 878.543 | 1339.214 | 992.000 |
| smalltext | in_band | 1024 | native | 452.535 | 332.428 | 689.552 | 446.000 |
| smalltext | in_band | 1024 | pg_backend | 1134.344 | 963.192 | 1483.158 | 1112.000 |
| smalltext | in_band | 1024 | pg_copy | 1281.473 | 984.132 | 1861.069 | 1137.000 |
| smalltext | in_band | 4096 | direct | 814.364 | 724.272 | 979.150 | 811.000 |
| smalltext | in_band | 4096 | native | 185.519 | 180.308 | 306.806 | 181.000 |
| smalltext | in_band | 4096 | pg_backend | 972.817 | 770.533 | 1164.447 | 945.000 |
| smalltext | in_band | 4096 | pg_copy | 1193.752 | 935.155 | 1807.857 | 1020.000 |
| smalltext | in_band | 16384 | direct | 769.047 | 702.290 | 935.747 | 765.000 |
| smalltext | in_band | 16384 | native | 135.173 | 117.546 | 191.323 | 130.000 |
| smalltext | in_band | 16384 | pg_backend | 787.288 | 670.285 | 1077.825 | 746.000 |
| smalltext | in_band | 16384 | pg_copy | 1294.374 | 919.594 | 1676.758 | 1114.000 |

Native ProfileEvents medians (ms, aggregate across query threads where applicable; overlapping diagnostics). Missing event counters mean zero.

| workload | policy | block_size | route | network send/wait | read syscall | CPU scheduling wait | user CPU | system CPU |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | 1024 | direct | 92.275 | 1.853 | 9.411 | 69.801 | 153.964 |
| narrow | flush_first | 1024 | native | 89.843 | 1.840 | 13.218 | 118.473 | 159.398 |
| narrow | flush_first | 1024 | pg_backend | 84.683 | 1.858 | 59.137 | 70.048 | 124.691 |
| narrow | flush_first | 1024 | pg_copy | 112.332 | 1.897 | 64.512 | 83.273 | 121.265 |
| narrow | flush_first | 4096 | direct | 24.902 | 1.845 | 6.233 | 32.829 | 46.300 |
| narrow | flush_first | 4096 | native | 31.616 | 2.066 | 8.003 | 62.801 | 56.161 |
| narrow | flush_first | 4096 | pg_backend | 123.725 | 1.797 | 18.754 | 34.404 | 40.568 |
| narrow | flush_first | 4096 | pg_copy | 202.148 | 1.874 | 25.827 | 37.130 | 36.869 |
| narrow | flush_first | 16384 | direct | 12.643 | 1.666 | 4.691 | 24.578 | 22.684 |
| narrow | flush_first | 16384 | native | 10.125 | 1.765 | 2.237 | 50.786 | 22.454 |
| narrow | flush_first | 16384 | pg_backend | 142.734 | 1.803 | 9.067 | 27.283 | 18.297 |
| narrow | flush_first | 16384 | pg_copy | 255.343 | 1.899 | 21.141 | 27.546 | 18.209 |
| narrow | in_band | 1024 | direct | 91.182 | 1.893 | 16.307 | 189.770 | 164.336 |
| narrow | in_band | 1024 | native | 98.816 | 1.673 | 13.215 | 108.963 | 157.119 |
| narrow | in_band | 1024 | pg_backend | 93.267 | 1.660 | 30.539 | 184.713 | 150.397 |
| narrow | in_band | 1024 | pg_copy | 104.412 | 1.808 | 56.466 | 192.624 | 131.939 |
| narrow | in_band | 4096 | direct | 34.546 | 1.915 | 7.904 | 136.159 | 65.391 |
| narrow | in_band | 4096 | native | 27.203 | 1.843 | 5.189 | 58.313 | 49.877 |
| narrow | in_band | 4096 | pg_backend | 46.000 | 1.925 | 13.371 | 144.624 | 46.598 |
| narrow | in_band | 4096 | pg_copy | 119.675 | 1.821 | 32.166 | 156.802 | 46.238 |
| narrow | in_band | 16384 | direct | 16.420 | 1.702 | 5.958 | 129.924 | 35.311 |
| narrow | in_band | 16384 | native | 11.400 | 1.678 | 2.001 | 46.111 | 22.967 |
| narrow | in_band | 16384 | pg_backend | 69.693 | 1.810 | 17.484 | 134.406 | 21.166 |
| narrow | in_band | 16384 | pg_copy | 152.918 | 1.712 | 19.836 | 130.327 | 22.227 |
| smalltext | flush_first | 1024 | direct | 122.031 | 2.507 | 55.295 | 227.775 | 185.486 |
| smalltext | flush_first | 1024 | native | 101.738 | 2.238 | 68.436 | 250.196 | 178.757 |
| smalltext | flush_first | 1024 | pg_backend | 342.833 | 2.578 | 83.337 | 210.273 | 152.330 |
| smalltext | flush_first | 1024 | pg_copy | 807.770 | 2.230 | 91.028 | 211.400 | 153.356 |
| smalltext | flush_first | 4096 | direct | 59.845 | 2.275 | 15.043 | 142.623 | 95.571 |
| smalltext | flush_first | 4096 | native | 42.913 | 2.558 | 34.350 | 169.630 | 84.233 |
| smalltext | flush_first | 4096 | pg_backend | 312.971 | 2.170 | 26.548 | 137.626 | 80.457 |
| smalltext | flush_first | 4096 | pg_copy | 950.312 | 2.375 | 96.653 | 157.606 | 79.937 |
| smalltext | flush_first | 16384 | direct | 46.833 | 2.678 | 4.379 | 113.622 | 67.013 |
| smalltext | flush_first | 16384 | native | 17.605 | 2.111 | 2.999 | 132.582 | 41.530 |
| smalltext | flush_first | 16384 | pg_backend | 342.838 | 2.524 | 58.597 | 115.469 | 59.089 |
| smalltext | flush_first | 16384 | pg_copy | 926.932 | 2.614 | 61.872 | 115.963 | 49.732 |
| smalltext | in_band | 1024 | direct | 161.741 | 2.296 | 64.415 | 902.432 | 270.454 |
| smalltext | in_band | 1024 | native | 111.594 | 4.474 | 187.550 | 260.267 | 215.062 |
| smalltext | in_band | 1024 | pg_backend | 205.493 | 2.147 | 107.347 | 975.332 | 275.321 |
| smalltext | in_band | 1024 | pg_copy | 204.328 | 2.209 | 155.365 | 939.637 | 259.742 |
| smalltext | in_band | 4096 | direct | 82.139 | 2.414 | 47.810 | 741.324 | 146.110 |
| smalltext | in_band | 4096 | native | 40.892 | 2.248 | 10.019 | 177.284 | 85.693 |
| smalltext | in_band | 4096 | pg_backend | 116.766 | 2.311 | 227.452 | 780.531 | 123.428 |
| smalltext | in_band | 4096 | pg_copy | 242.143 | 2.242 | 208.114 | 803.351 | 105.048 |
| smalltext | in_band | 16384 | direct | 57.246 | 2.767 | 32.047 | 697.538 | 107.251 |
| smalltext | in_band | 16384 | native | 18.563 | 2.732 | 4.105 | 131.704 | 54.423 |
| smalltext | in_band | 16384 | pg_backend | 64.264 | 2.350 | 52.405 | 705.067 | 90.104 |
| smalltext | in_band | 16384 | pg_copy | 317.790 | 2.367 | 58.074 | 733.423 | 68.066 |

Overlapping direct-client diagnostics (ms); never add these clocks. Flush/observe are aggregate per operation.

| workload | policy | block_size | route | body bytes | body wait | Arrow decode | framing decode | terminal | flush | observe | poll wait | poll count range |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | 1024 | direct | 8127712 | 147.496 | 12.173 | 0.000 | 34.809 | 19.505 | 17.519 | 0.000 | [1, 1] |
| narrow | flush_first | 4096 | direct | 8033536 | 48.574 | 4.296 | 0.000 | 32.149 | 16.599 | 17.918 | 0.000 | [1, 1] |
| narrow | flush_first | 16384 | direct | 8009200 | 28.329 | 3.015 | 0.000 | 37.628 | 18.787 | 19.577 | 0.000 | [1, 1] |
| narrow | in_band | 1024 | direct | 10886599 | 234.852 | 9.845 | 14.070 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| narrow | in_band | 4096 | direct | 10737191 | 132.224 | 5.259 | 12.666 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| narrow | in_band | 16384 | direct | 10699630 | 114.849 | 2.091 | 12.416 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| smalltext | flush_first | 1024 | direct | 50256168 | 265.307 | 27.172 | 0.000 | 54.324 | 24.512 | 25.794 | 0.000 | [1, 1] |
| smalltext | flush_first | 4096 | direct | 50067816 | 124.265 | 25.295 | 0.000 | 32.094 | 16.654 | 17.238 | 0.000 | [1, 1] |
| smalltext | flush_first | 16384 | direct | 50018832 | 83.206 | 34.467 | 0.000 | 30.201 | 14.676 | 16.055 | 0.000 | [1, 1] |
| smalltext | in_band | 1024 | direct | 67090125 | 786.471 | 35.073 | 73.253 | 0.004 | 0.000 | 0.000 | 0.000 | [0, 0] |
| smalltext | in_band | 4096 | direct | 66811980 | 656.919 | 16.707 | 76.432 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| smalltext | in_band | 16384 | direct | 66744721 | 604.954 | 9.358 | 80.107 | 0.004 | 0.000 | 0.000 | 0.000 | [0, 0] |

## native-uncompressed

| workload | policy | block_size | route | median ms | min ms | max ms | server median ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | 1024 | native | 172.991 | 163.255 | 186.430 | 170.000 |
| narrow | flush_first | 4096 | native | 55.743 | 50.872 | 61.171 | 52.000 |
| narrow | flush_first | 16384 | native | 26.391 | 25.446 | 32.672 | 22.000 |
| smalltext | flush_first | 1024 | native | 302.727 | 275.352 | 326.021 | 260.000 |
| smalltext | flush_first | 4096 | native | 179.641 | 174.581 | 200.582 | 141.000 |
| smalltext | flush_first | 16384 | native | 149.647 | 141.920 | 165.540 | 106.000 |

Native ProfileEvents medians (ms, aggregate across query threads where applicable; overlapping diagnostics). Missing event counters mean zero.

| workload | policy | block_size | route | network send/wait | read syscall | CPU scheduling wait | user CPU | system CPU |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | 1024 | native | 88.031 | 1.242 | 1.831 | 70.700 | 154.638 |
| narrow | flush_first | 4096 | native | 25.743 | 1.230 | 0.286 | 32.235 | 44.185 |
| narrow | flush_first | 16384 | native | 9.711 | 1.062 | 0.733 | 21.995 | 16.283 |
| smalltext | flush_first | 1024 | native | 110.655 | 1.471 | 3.043 | 164.781 | 197.886 |
| smalltext | flush_first | 4096 | native | 51.344 | 1.707 | 2.474 | 125.296 | 90.429 |
| smalltext | flush_first | 16384 | native | 37.097 | 1.870 | 0.939 | 97.691 | 62.635 |

## streaming-wide-inband

| workload | policy | block_size | route | median ms | min ms | max ms | server median ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| wide1k | in_band | 1024 | direct | 726.004 | 643.031 | 893.420 | 704.000 |
| wide1k | in_band | 1024 | native | 186.783 | 113.576 | 326.841 | 100.000 |
| wide1k | in_band | 1024 | pg_backend | 702.741 | 614.086 | 821.412 | 653.000 |
| wide1k | in_band | 1024 | pg_copy | 891.060 | 812.989 | 974.794 | 668.000 |
| wide1k | in_band | 4096 | direct | 741.496 | 472.032 | 943.555 | 721.000 |
| wide1k | in_band | 4096 | native | 179.536 | 119.571 | 585.430 | 117.000 |
| wide1k | in_band | 4096 | pg_backend | 634.095 | 532.263 | 776.771 | 586.000 |
| wide1k | in_band | 4096 | pg_copy | 790.220 | 605.710 | 1084.123 | 663.000 |
| wide1k | in_band | 16384 | direct | 729.963 | 505.185 | 810.928 | 696.000 |
| wide1k | in_band | 16384 | native | 140.648 | 83.671 | 350.939 | 99.000 |
| wide1k | in_band | 16384 | pg_backend | 703.920 | 503.865 | 819.186 | 659.000 |
| wide1k | in_band | 16384 | pg_copy | 821.871 | 528.422 | 1025.927 | 694.000 |

Native ProfileEvents medians (ms, aggregate across query threads where applicable; overlapping diagnostics). Missing event counters mean zero.

| workload | policy | block_size | route | network send/wait | read syscall | CPU scheduling wait | user CPU | system CPU |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| wide1k | in_band | 1024 | direct | 142.086 | 0.246 | 60.026 | 493.573 | 123.946 |
| wide1k | in_band | 1024 | native | 52.965 | 0.227 | 18.343 | 54.999 | 36.601 |
| wide1k | in_band | 1024 | pg_backend | 60.949 | 0.223 | 100.442 | 496.444 | 121.687 |
| wide1k | in_band | 1024 | pg_copy | 68.118 | 0.235 | 79.273 | 498.476 | 101.678 |
| wide1k | in_band | 4096 | direct | 83.714 | 0.217 | 62.925 | 493.852 | 148.501 |
| wide1k | in_band | 4096 | native | 40.623 | 0.257 | 34.595 | 48.898 | 44.915 |
| wide1k | in_band | 4096 | pg_backend | 50.487 | 0.217 | 49.524 | 490.868 | 74.782 |
| wide1k | in_band | 4096 | pg_copy | 64.494 | 0.236 | 66.678 | 506.803 | 83.246 |
| wide1k | in_band | 16384 | direct | 96.453 | 0.253 | 28.231 | 500.377 | 124.952 |
| wide1k | in_band | 16384 | native | 36.102 | 0.230 | 13.524 | 46.410 | 45.598 |
| wide1k | in_band | 16384 | pg_backend | 81.459 | 0.257 | 86.678 | 509.022 | 88.691 |
| wide1k | in_band | 16384 | pg_copy | 71.315 | 0.253 | 94.796 | 512.503 | 116.045 |

Overlapping direct-client diagnostics (ms); never add these clocks. Flush/observe are aggregate per operation.

| workload | policy | block_size | route | body bytes | body wait | Arrow decode | framing decode | terminal | flush | observe | poll wait | poll count range |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| wide1k | in_band | 1024 | direct | 46252357 | 586.126 | 0.430 | 104.768 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| wide1k | in_band | 4096 | direct | 46252505 | 643.421 | 0.406 | 72.750 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| wide1k | in_band | 16384 | direct | 46252496 | 598.670 | 0.431 | 84.416 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |

## compression-paired

| workload | policy | codec | median ms | min ms | max ms | server median ms |
| --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | none | 115.765 | 101.209 | 137.956 | 65.000 |
| narrow | flush_first | zstd | 195.560 | 164.716 | 218.334 | 140.000 |
| narrow | in_band | none | 223.377 | 197.101 | 237.136 | 219.000 |
| narrow | in_band | zstd | 152.071 | 149.013 | 179.990 | 149.000 |
| smalltext | flush_first | none | 239.498 | 219.441 | 318.118 | 198.000 |
| smalltext | flush_first | zstd | 375.747 | 278.664 | 730.291 | 334.000 |
| smalltext | in_band | none | 991.234 | 896.088 | 1143.709 | 988.000 |
| smalltext | in_band | zstd | 371.720 | 352.841 | 446.457 | 369.000 |
| wide1k | in_band | none | 642.348 | 594.704 | 684.250 | 624.000 |
| wide1k | in_band | zstd | 67.080 | 60.391 | 87.427 | 64.000 |

Native ProfileEvents medians (ms, aggregate across query threads where applicable; overlapping diagnostics). Missing event counters mean zero.

| workload | policy | codec | network send/wait | read syscall | CPU scheduling wait | user CPU | system CPU |
| --- | --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | none | 29.631 | 1.218 | 24.019 | 33.611 | 43.717 |
| narrow | flush_first | zstd | 36.482 | 1.289 | 102.832 | 92.807 | 49.994 |
| narrow | in_band | none | 45.630 | 1.271 | 110.672 | 160.977 | 59.069 |
| narrow | in_band | zstd | 34.276 | 1.297 | 53.104 | 111.718 | 49.581 |
| smalltext | flush_first | none | 67.737 | 1.655 | 98.618 | 135.778 | 93.411 |
| smalltext | flush_first | zstd | 63.552 | 1.592 | 222.845 | 219.393 | 91.247 |
| smalltext | in_band | none | 118.820 | 2.175 | 380.035 | 790.523 | 125.533 |
| smalltext | in_band | zstd | 51.280 | 1.654 | 146.236 | 298.467 | 87.776 |
| wide1k | in_band | none | 35.474 | 0.198 | 32.253 | 492.770 | 108.838 |
| wide1k | in_band | zstd | 0.935 | 0.170 | 15.840 | 51.461 | 25.744 |

Overlapping direct-client diagnostics (ms); never add these clocks. Flush/observe are aggregate per operation.

| workload | policy | codec | body bytes | body wait | Arrow decode | framing decode | terminal | flush | observe | poll wait | poll count range |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| narrow | flush_first | none | 8034688 | 55.577 | 4.983 | 0.000 | 39.431 | 18.133 | 21.257 | 0.000 | [1, 1] |
| narrow | flush_first | zstd | 1068152 | 95.365 | 34.997 | 0.000 | 37.415 | 18.398 | 20.595 | 0.000 | [1, 1] |
| narrow | in_band | none | 10743437 | 178.389 | 5.750 | 14.938 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| narrow | in_band | zstd | 1450352 | 96.502 | 34.243 | 3.388 | 0.004 | 0.000 | 0.000 | 0.000 | [0, 0] |
| smalltext | flush_first | none | 50070120 | 166.943 | 22.177 | 0.000 | 39.870 | 20.188 | 19.639 | 0.000 | [1, 1] |
| smalltext | flush_first | zstd | 5856328 | 227.843 | 85.239 | 0.000 | 39.012 | 19.192 | 20.844 | 0.000 | [1, 1] |
| smalltext | in_band | none | 66824138 | 805.591 | 17.817 | 85.536 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| smalltext | in_band | zstd | 7845394 | 258.786 | 80.062 | 12.886 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| wide1k | in_band | none | 46252476 | 536.440 | 0.435 | 61.047 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| wide1k | in_band | zstd | 280857 | 58.135 | 8.360 | 0.587 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |

## terminal-1000

| workload | policy | block_size | route | median ms | min ms | max ms | server median ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| lake_full | flush_first | 1024 | direct | 56.413 | 48.821 | 72.605 | 20.000 |
| lake_full | flush_first | 1024 | native | 23.920 | 19.785 | 30.677 | 24.000 |
| lake_full | flush_first | 1024 | pg_backend | 58.025 | 54.869 | 63.153 | 22.000 |
| lake_full | flush_first | 1024 | pg_copy | 57.645 | 48.816 | 59.537 | 21.000 |
| lake_full | in_band | 1024 | direct | 31.734 | 22.067 | 36.528 | 27.000 |
| lake_full | in_band | 1024 | native | 25.134 | 23.468 | 31.813 | 25.000 |
| lake_full | in_band | 1024 | pg_backend | 29.487 | 24.907 | 36.221 | 22.000 |
| lake_full | in_band | 1024 | pg_copy | 29.533 | 23.510 | 35.340 | 25.000 |
| lake_full | observe_first | 1024 | direct | 66.149 | 61.914 | 77.288 | 20.000 |
| lake_full | observe_first | 1024 | native | 23.312 | 21.830 | 29.338 | 22.000 |
| lake_full | observe_first | 1024 | pg_backend | 75.019 | 68.209 | 79.230 | 24.000 |
| lake_full | observe_first | 1024 | pg_copy | 69.105 | 67.281 | 74.911 | 19.000 |
| lake_full | observe_only | 1024 | direct | 1055.349 | 1046.653 | 1090.840 | 20.000 |
| lake_full | observe_only | 1024 | native | 22.422 | 21.622 | 26.542 | 20.000 |
| lake_full | observe_only | 1024 | pg_backend | 978.918 | 947.219 | 1071.268 | 21.000 |
| lake_full | observe_only | 1024 | pg_copy | 969.900 | 943.941 | 1061.748 | 24.000 |
| lake_indexed | flush_first | 1024 | direct | 48.520 | 43.390 | 50.868 | 13.000 |
| lake_indexed | flush_first | 1024 | native | 16.527 | 15.470 | 19.612 | 14.000 |
| lake_indexed | flush_first | 1024 | pg_backend | 50.479 | 48.445 | 53.174 | 14.000 |
| lake_indexed | flush_first | 1024 | pg_copy | 52.603 | 46.870 | 56.658 | 15.000 |
| lake_indexed | in_band | 1024 | direct | 20.882 | 16.066 | 33.567 | 17.000 |
| lake_indexed | in_band | 1024 | native | 20.296 | 16.228 | 33.302 | 17.000 |
| lake_indexed | in_band | 1024 | pg_backend | 21.908 | 20.210 | 35.790 | 15.000 |
| lake_indexed | in_band | 1024 | pg_copy | 21.459 | 18.859 | 22.713 | 15.000 |
| lake_indexed | observe_first | 1024 | direct | 74.714 | 59.401 | 103.125 | 14.000 |
| lake_indexed | observe_first | 1024 | native | 19.469 | 13.367 | 21.123 | 17.000 |
| lake_indexed | observe_first | 1024 | pg_backend | 70.144 | 67.515 | 113.487 | 15.000 |
| lake_indexed | observe_first | 1024 | pg_copy | 78.631 | 62.787 | 101.703 | 19.000 |
| lake_indexed | observe_only | 1024 | direct | 1089.572 | 999.996 | 1111.057 | 18.000 |
| lake_indexed | observe_only | 1024 | native | 21.106 | 17.043 | 26.380 | 19.000 |
| lake_indexed | observe_only | 1024 | pg_backend | 1021.592 | 969.496 | 1067.086 | 17.000 |
| lake_indexed | observe_only | 1024 | pg_copy | 981.278 | 948.862 | 1100.709 | 21.000 |
| small | flush_first | 1024 | direct | 38.256 | 34.910 | 40.107 | 3.000 |
| small | flush_first | 1024 | native | 6.354 | 4.822 | 9.398 | 3.000 |
| small | flush_first | 1024 | pg_backend | 37.671 | 36.603 | 39.364 | 4.000 |
| small | flush_first | 1024 | pg_copy | 38.539 | 34.327 | 46.656 | 4.000 |
| small | in_band | 1024 | direct | 6.739 | 6.642 | 7.513 | 4.000 |
| small | in_band | 1024 | native | 5.466 | 5.030 | 5.603 | 3.000 |
| small | in_band | 1024 | pg_backend | 10.260 | 7.985 | 11.633 | 4.000 |
| small | in_band | 1024 | pg_copy | 8.182 | 7.442 | 11.541 | 4.000 |
| small | observe_first | 1024 | direct | 57.498 | 52.061 | 64.364 | 3.000 |
| small | observe_first | 1024 | native | 5.526 | 4.761 | 7.960 | 3.000 |
| small | observe_first | 1024 | pg_backend | 54.860 | 51.710 | 58.209 | 4.000 |
| small | observe_first | 1024 | pg_copy | 55.485 | 50.707 | 66.450 | 3.000 |
| small | observe_only | 1024 | direct | 1056.319 | 1036.979 | 1067.015 | 4.000 |
| small | observe_only | 1024 | native | 5.840 | 4.837 | 8.846 | 4.000 |
| small | observe_only | 1024 | pg_backend | 1029.938 | 959.471 | 1054.143 | 4.000 |
| small | observe_only | 1024 | pg_copy | 1044.479 | 932.567 | 1063.749 | 4.000 |

Native ProfileEvents medians (ms, aggregate across query threads where applicable; overlapping diagnostics). Missing event counters mean zero.

| workload | policy | block_size | route | network send/wait | read syscall | CPU scheduling wait | user CPU | system CPU |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| lake_full | flush_first | 1024 | direct | 0.768 | 0.323 | 4.745 | 24.877 | 9.809 |
| lake_full | flush_first | 1024 | native | 0.719 | 0.374 | 2.753 | 26.198 | 7.556 |
| lake_full | flush_first | 1024 | pg_backend | 0.742 | 0.331 | 6.393 | 24.443 | 9.992 |
| lake_full | flush_first | 1024 | pg_copy | 0.522 | 0.357 | 8.657 | 24.828 | 7.344 |
| lake_full | in_band | 1024 | direct | 0.927 | 0.416 | 3.158 | 29.750 | 9.070 |
| lake_full | in_band | 1024 | native | 0.733 | 0.368 | 7.127 | 28.098 | 10.567 |
| lake_full | in_band | 1024 | pg_backend | 0.655 | 0.320 | 8.192 | 25.114 | 7.861 |
| lake_full | in_band | 1024 | pg_copy | 0.518 | 0.395 | 4.213 | 25.401 | 8.036 |
| lake_full | observe_first | 1024 | direct | 0.740 | 0.371 | 6.470 | 23.549 | 7.013 |
| lake_full | observe_first | 1024 | native | 0.743 | 0.365 | 7.381 | 28.495 | 7.364 |
| lake_full | observe_first | 1024 | pg_backend | 0.697 | 0.339 | 3.417 | 25.062 | 8.705 |
| lake_full | observe_first | 1024 | pg_copy | 0.581 | 0.376 | 3.295 | 24.815 | 7.534 |
| lake_full | observe_only | 1024 | direct | 0.720 | 0.321 | 3.972 | 26.026 | 6.491 |
| lake_full | observe_only | 1024 | native | 0.769 | 0.304 | 6.470 | 23.260 | 10.010 |
| lake_full | observe_only | 1024 | pg_backend | 0.752 | 0.330 | 2.955 | 24.627 | 14.313 |
| lake_full | observe_only | 1024 | pg_copy | 0.655 | 0.494 | 5.757 | 24.619 | 10.081 |
| lake_indexed | flush_first | 1024 | direct | 0.721 | 0.234 | 1.562 | 12.296 | 1.956 |
| lake_indexed | flush_first | 1024 | native | 0.807 | 0.261 | 2.125 | 11.198 | 4.945 |
| lake_indexed | flush_first | 1024 | pg_backend | 0.738 | 0.242 | 1.633 | 11.621 | 3.946 |
| lake_indexed | flush_first | 1024 | pg_copy | 0.566 | 0.223 | 1.909 | 11.401 | 3.990 |
| lake_indexed | in_band | 1024 | direct | 0.747 | 0.659 | 2.405 | 11.538 | 6.284 |
| lake_indexed | in_band | 1024 | native | 0.752 | 0.749 | 3.487 | 13.924 | 4.834 |
| lake_indexed | in_band | 1024 | pg_backend | 0.669 | 0.296 | 1.276 | 12.984 | 4.151 |
| lake_indexed | in_band | 1024 | pg_copy | 0.576 | 0.316 | 2.532 | 12.067 | 5.395 |
| lake_indexed | observe_first | 1024 | direct | 0.748 | 0.253 | 4.054 | 10.540 | 4.646 |
| lake_indexed | observe_first | 1024 | native | 0.740 | 0.224 | 3.992 | 10.078 | 3.266 |
| lake_indexed | observe_first | 1024 | pg_backend | 0.956 | 0.305 | 2.476 | 11.645 | 4.199 |
| lake_indexed | observe_first | 1024 | pg_copy | 0.655 | 0.210 | 8.413 | 11.860 | 2.988 |
| lake_indexed | observe_only | 1024 | direct | 0.868 | 0.190 | 6.677 | 13.645 | 2.854 |
| lake_indexed | observe_only | 1024 | native | 0.914 | 0.216 | 3.880 | 12.828 | 4.096 |
| lake_indexed | observe_only | 1024 | pg_backend | 0.891 | 0.234 | 8.508 | 14.235 | 4.277 |
| lake_indexed | observe_only | 1024 | pg_copy | 0.780 | 0.653 | 7.438 | 14.778 | 6.134 |
| small | flush_first | 1024 | direct | 0.482 | 0.015 | 0.101 | 2.556 | 0.911 |
| small | flush_first | 1024 | native | 0.482 | 0.015 | 0.000 | 2.368 | 1.927 |
| small | flush_first | 1024 | pg_backend | 0.493 | 0.015 | 0.008 | 1.692 | 1.828 |
| small | flush_first | 1024 | pg_copy | 0.372 | 0.016 | 0.235 | 2.604 | 1.349 |
| small | in_band | 1024 | direct | 0.490 | 0.015 | 0.000 | 2.985 | 0.993 |
| small | in_band | 1024 | native | 0.485 | 0.016 | 0.042 | 2.343 | 0.880 |
| small | in_band | 1024 | pg_backend | 0.658 | 0.015 | 0.171 | 2.685 | 0.915 |
| small | in_band | 1024 | pg_copy | 0.360 | 0.015 | 0.000 | 2.501 | 1.140 |
| small | observe_first | 1024 | direct | 0.481 | 0.015 | 0.000 | 2.246 | 1.742 |
| small | observe_first | 1024 | native | 0.489 | 0.014 | 0.083 | 2.180 | 0.858 |
| small | observe_first | 1024 | pg_backend | 0.489 | 0.015 | 0.476 | 2.977 | 0.988 |
| small | observe_first | 1024 | pg_copy | 0.410 | 0.015 | 0.000 | 1.924 | 0.864 |
| small | observe_only | 1024 | direct | 0.954 | 0.015 | 0.327 | 2.017 | 1.834 |
| small | observe_only | 1024 | native | 0.542 | 0.016 | 0.028 | 1.658 | 1.863 |
| small | observe_only | 1024 | pg_backend | 0.477 | 0.014 | 0.020 | 2.159 | 0.950 |
| small | observe_only | 1024 | pg_copy | 0.481 | 0.016 | 0.059 | 3.150 | 0.775 |

Overlapping direct-client diagnostics (ms); never add these clocks. Flush/observe are aggregate per operation.

| workload | policy | block_size | route | body bytes | body wait | Arrow decode | framing decode | terminal | flush | observe | poll wait | poll count range |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| lake_full | flush_first | 1024 | direct | 7344 | 21.494 | 0.113 | 0.000 | 31.621 | 16.888 | 14.871 | 0.000 | [1, 1] |
| lake_full | in_band | 1024 | direct | 20590 | 31.001 | 0.108 | 0.285 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| lake_full | observe_first | 1024 | direct | 7344 | 21.758 | 0.134 | 0.000 | 44.043 | 14.836 | 28.939 | 0.000 | [2, 2] |
| lake_full | observe_only | 1024 | direct | 7344 | 20.909 | 0.102 | 0.000 | 1034.061 | 0.000 | 171.798 | 861.935 | [12, 12] |
| lake_indexed | flush_first | 1024 | direct | 7344 | 13.650 | 0.099 | 0.000 | 33.469 | 15.991 | 16.239 | 0.000 | [1, 1] |
| lake_indexed | in_band | 1024 | direct | 21157 | 20.148 | 0.125 | 0.298 | 0.004 | 0.000 | 0.000 | 0.000 | [0, 0] |
| lake_indexed | observe_first | 1024 | direct | 7344 | 16.431 | 0.116 | 0.000 | 57.283 | 17.911 | 39.307 | 0.000 | [2, 2] |
| lake_indexed | observe_only | 1024 | direct | 7344 | 19.407 | 0.101 | 0.000 | 1045.568 | 0.000 | 225.320 | 862.753 | [11, 12] |
| small | flush_first | 1024 | direct | 6984 | 5.012 | 0.069 | 0.000 | 32.777 | 15.046 | 17.043 | 0.000 | [1, 1] |
| small | in_band | 1024 | direct | 20317 | 6.203 | 0.062 | 0.252 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| small | observe_first | 1024 | direct | 6984 | 5.033 | 0.063 | 0.000 | 52.235 | 17.238 | 36.051 | 0.000 | [2, 2] |
| small | observe_only | 1024 | direct | 6984 | 6.321 | 0.073 | 0.000 | 1049.096 | 0.000 | 185.954 | 862.316 | [12, 12] |

## terminal-100

| workload | policy | block_size | route | median ms | min ms | max ms | server median ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| lake_full | flush_first | 1024 | direct | 97.603 | 83.884 | 136.843 | 41.000 |
| lake_full | flush_first | 1024 | native | 43.285 | 38.823 | 77.129 | 37.000 |
| lake_full | flush_first | 1024 | pg_backend | 135.371 | 83.462 | 246.182 | 44.000 |
| lake_full | flush_first | 1024 | pg_copy | 107.263 | 93.331 | 142.967 | 43.000 |
| lake_full | in_band | 1024 | direct | 39.579 | 37.076 | 57.293 | 36.000 |
| lake_full | in_band | 1024 | native | 44.149 | 40.820 | 74.519 | 39.000 |
| lake_full | in_band | 1024 | pg_backend | 47.940 | 45.532 | 54.665 | 38.000 |
| lake_full | in_band | 1024 | pg_copy | 47.072 | 42.515 | 64.681 | 41.000 |
| lake_full | observe_first | 1024 | direct | 132.129 | 107.401 | 138.117 | 34.000 |
| lake_full | observe_first | 1024 | native | 42.477 | 31.471 | 63.236 | 36.000 |
| lake_full | observe_first | 1024 | pg_backend | 139.093 | 106.071 | 152.927 | 36.000 |
| lake_full | observe_first | 1024 | pg_copy | 125.663 | 104.760 | 158.732 | 36.000 |
| lake_full | observe_only | 1024 | direct | 219.991 | 191.881 | 237.344 | 42.000 |
| lake_full | observe_only | 1024 | native | 40.552 | 35.164 | 71.813 | 34.000 |
| lake_full | observe_only | 1024 | pg_backend | 218.765 | 129.043 | 247.841 | 36.000 |
| lake_full | observe_only | 1024 | pg_copy | 223.993 | 118.960 | 256.184 | 42.000 |
| lake_indexed | flush_first | 1024 | direct | 86.037 | 73.014 | 95.257 | 23.000 |
| lake_indexed | flush_first | 1024 | native | 27.160 | 25.395 | 28.932 | 18.000 |
| lake_indexed | flush_first | 1024 | pg_backend | 90.991 | 73.781 | 262.396 | 21.000 |
| lake_indexed | flush_first | 1024 | pg_copy | 89.069 | 78.917 | 243.998 | 21.000 |
| lake_indexed | in_band | 1024 | direct | 22.630 | 22.027 | 36.208 | 19.000 |
| lake_indexed | in_band | 1024 | native | 25.659 | 21.010 | 27.447 | 21.000 |
| lake_indexed | in_band | 1024 | pg_backend | 30.900 | 28.066 | 35.800 | 22.000 |
| lake_indexed | in_band | 1024 | pg_copy | 31.622 | 26.473 | 45.612 | 24.000 |
| lake_indexed | observe_first | 1024 | direct | 105.715 | 91.816 | 164.389 | 24.000 |
| lake_indexed | observe_first | 1024 | native | 25.976 | 24.534 | 33.035 | 23.000 |
| lake_indexed | observe_first | 1024 | pg_backend | 109.814 | 100.089 | 132.354 | 25.000 |
| lake_indexed | observe_first | 1024 | pg_copy | 106.275 | 92.304 | 119.984 | 24.000 |
| lake_indexed | observe_only | 1024 | direct | 98.361 | 83.851 | 193.620 | 22.000 |
| lake_indexed | observe_only | 1024 | native | 29.567 | 21.994 | 63.680 | 20.000 |
| lake_indexed | observe_only | 1024 | pg_backend | 152.681 | 139.342 | 214.409 | 23.000 |
| lake_indexed | observe_only | 1024 | pg_copy | 133.715 | 83.571 | 241.903 | 22.000 |
| small | flush_first | 1024 | direct | 62.489 | 50.728 | 72.514 | 4.000 |
| small | flush_first | 1024 | native | 15.116 | 10.286 | 17.715 | 6.000 |
| small | flush_first | 1024 | pg_backend | 64.079 | 56.139 | 96.092 | 6.000 |
| small | flush_first | 1024 | pg_copy | 72.609 | 60.265 | 222.959 | 7.000 |
| small | in_band | 1024 | direct | 12.082 | 8.493 | 26.431 | 8.000 |
| small | in_band | 1024 | native | 10.941 | 8.358 | 18.601 | 5.000 |
| small | in_band | 1024 | pg_backend | 24.774 | 12.834 | 36.019 | 12.000 |
| small | in_band | 1024 | pg_copy | 14.791 | 12.331 | 24.546 | 6.000 |
| small | observe_first | 1024 | direct | 95.647 | 89.547 | 151.857 | 5.000 |
| small | observe_first | 1024 | native | 13.537 | 7.824 | 19.099 | 6.000 |
| small | observe_first | 1024 | pg_backend | 89.870 | 89.207 | 115.345 | 7.000 |
| small | observe_first | 1024 | pg_copy | 95.116 | 89.765 | 112.596 | 6.000 |
| small | observe_only | 1024 | direct | 117.364 | 87.122 | 122.121 | 7.000 |
| small | observe_only | 1024 | native | 13.809 | 12.129 | 19.316 | 7.000 |
| small | observe_only | 1024 | pg_backend | 120.928 | 113.137 | 154.350 | 6.000 |
| small | observe_only | 1024 | pg_copy | 129.679 | 117.797 | 132.984 | 7.000 |

Native ProfileEvents medians (ms, aggregate across query threads where applicable; overlapping diagnostics). Missing event counters mean zero.

| workload | policy | block_size | route | network send/wait | read syscall | CPU scheduling wait | user CPU | system CPU |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| lake_full | flush_first | 1024 | direct | 0.853 | 0.357 | 52.755 | 29.443 | 10.456 |
| lake_full | flush_first | 1024 | native | 0.833 | 0.405 | 52.132 | 28.542 | 8.259 |
| lake_full | flush_first | 1024 | pg_backend | 0.931 | 0.345 | 71.915 | 28.779 | 11.545 |
| lake_full | flush_first | 1024 | pg_copy | 1.371 | 0.427 | 79.182 | 31.343 | 7.494 |
| lake_full | in_band | 1024 | direct | 0.766 | 0.358 | 45.449 | 30.821 | 8.647 |
| lake_full | in_band | 1024 | native | 0.896 | 0.335 | 57.069 | 29.785 | 11.945 |
| lake_full | in_band | 1024 | pg_backend | 0.611 | 0.356 | 47.918 | 32.740 | 7.009 |
| lake_full | in_band | 1024 | pg_copy | 0.595 | 0.409 | 52.756 | 30.731 | 10.013 |
| lake_full | observe_first | 1024 | direct | 0.824 | 0.393 | 33.579 | 29.878 | 8.356 |
| lake_full | observe_first | 1024 | native | 0.888 | 0.390 | 40.783 | 32.186 | 8.620 |
| lake_full | observe_first | 1024 | pg_backend | 0.892 | 0.400 | 40.087 | 29.640 | 8.320 |
| lake_full | observe_first | 1024 | pg_copy | 1.035 | 0.374 | 50.641 | 30.173 | 8.322 |
| lake_full | observe_only | 1024 | direct | 0.851 | 0.338 | 67.980 | 31.906 | 7.011 |
| lake_full | observe_only | 1024 | native | 0.831 | 0.402 | 40.206 | 29.843 | 10.904 |
| lake_full | observe_only | 1024 | pg_backend | 1.002 | 0.374 | 43.675 | 28.712 | 10.018 |
| lake_full | observe_only | 1024 | pg_copy | 1.200 | 0.355 | 62.022 | 31.729 | 8.764 |
| lake_indexed | flush_first | 1024 | direct | 0.850 | 0.282 | 10.350 | 13.341 | 5.197 |
| lake_indexed | flush_first | 1024 | native | 0.905 | 0.256 | 8.867 | 14.043 | 3.702 |
| lake_indexed | flush_first | 1024 | pg_backend | 1.396 | 0.270 | 10.035 | 13.181 | 3.991 |
| lake_indexed | flush_first | 1024 | pg_copy | 1.262 | 0.275 | 8.883 | 13.000 | 4.019 |
| lake_indexed | in_band | 1024 | direct | 0.755 | 0.338 | 8.588 | 12.968 | 4.999 |
| lake_indexed | in_band | 1024 | native | 0.874 | 0.269 | 7.229 | 12.944 | 5.269 |
| lake_indexed | in_band | 1024 | pg_backend | 0.585 | 0.259 | 11.122 | 13.016 | 4.070 |
| lake_indexed | in_band | 1024 | pg_copy | 0.568 | 0.327 | 11.004 | 13.530 | 3.847 |
| lake_indexed | observe_first | 1024 | direct | 0.905 | 0.308 | 9.665 | 14.595 | 5.163 |
| lake_indexed | observe_first | 1024 | native | 0.919 | 0.321 | 12.429 | 13.141 | 5.180 |
| lake_indexed | observe_first | 1024 | pg_backend | 1.672 | 0.276 | 14.744 | 13.524 | 4.900 |
| lake_indexed | observe_first | 1024 | pg_copy | 1.487 | 0.457 | 12.384 | 14.061 | 6.173 |
| lake_indexed | observe_only | 1024 | direct | 0.933 | 0.260 | 10.590 | 13.164 | 4.716 |
| lake_indexed | observe_only | 1024 | native | 0.899 | 0.297 | 7.770 | 13.741 | 4.019 |
| lake_indexed | observe_only | 1024 | pg_backend | 0.939 | 0.272 | 16.138 | 13.278 | 5.787 |
| lake_indexed | observe_only | 1024 | pg_copy | 2.044 | 0.273 | 9.588 | 14.321 | 4.324 |
| small | flush_first | 1024 | direct | 0.564 | 0.018 | 0.116 | 3.462 | 0.873 |
| small | flush_first | 1024 | native | 0.563 | 0.019 | 1.458 | 2.848 | 2.141 |
| small | flush_first | 1024 | pg_backend | 1.755 | 0.018 | 1.166 | 4.187 | 0.910 |
| small | flush_first | 1024 | pg_copy | 1.001 | 0.018 | 1.755 | 3.061 | 2.129 |
| small | in_band | 1024 | direct | 0.501 | 0.018 | 3.145 | 3.707 | 1.170 |
| small | in_band | 1024 | native | 0.528 | 0.020 | 0.182 | 3.567 | 0.969 |
| small | in_band | 1024 | pg_backend | 4.656 | 0.019 | 6.647 | 3.861 | 0.974 |
| small | in_band | 1024 | pg_copy | 1.648 | 0.017 | 1.491 | 3.482 | 1.789 |
| small | observe_first | 1024 | direct | 0.607 | 0.018 | 0.100 | 3.492 | 0.853 |
| small | observe_first | 1024 | native | 0.554 | 0.017 | 1.244 | 3.542 | 1.055 |
| small | observe_first | 1024 | pg_backend | 1.775 | 0.018 | 2.644 | 3.222 | 2.141 |
| small | observe_first | 1024 | pg_copy | 1.993 | 0.017 | 1.510 | 3.111 | 1.880 |
| small | observe_only | 1024 | direct | 0.543 | 0.019 | 2.227 | 2.887 | 1.097 |
| small | observe_only | 1024 | native | 0.692 | 0.019 | 2.756 | 3.108 | 1.946 |
| small | observe_only | 1024 | pg_backend | 1.407 | 0.019 | 0.852 | 2.744 | 2.155 |
| small | observe_only | 1024 | pg_copy | 0.695 | 0.018 | 2.664 | 2.978 | 1.977 |

Overlapping direct-client diagnostics (ms); never add these clocks. Flush/observe are aggregate per operation.

| workload | policy | block_size | route | body bytes | body wait | Arrow decode | framing decode | terminal | flush | observe | poll wait | poll count range |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| lake_full | flush_first | 1024 | direct | 7344 | 44.528 | 0.147 | 0.000 | 61.807 | 30.786 | 30.974 | 0.000 | [1, 1] |
| lake_full | in_band | 1024 | direct | 20593 | 38.888 | 0.111 | 0.290 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| lake_full | observe_first | 1024 | direct | 7344 | 38.592 | 0.132 | 0.000 | 93.111 | 27.753 | 52.766 | 0.000 | [2, 2] |
| lake_full | observe_only | 1024 | direct | 7344 | 43.669 | 0.128 | 0.000 | 180.070 | 0.000 | 104.591 | 73.506 | [3, 4] |
| lake_indexed | flush_first | 1024 | direct | 7344 | 26.912 | 0.118 | 0.000 | 59.639 | 26.332 | 32.751 | 0.000 | [1, 1] |
| lake_indexed | in_band | 1024 | direct | 21155 | 21.770 | 0.120 | 0.339 | 0.003 | 0.000 | 0.000 | 0.000 | [0, 0] |
| lake_indexed | observe_first | 1024 | direct | 7344 | 25.134 | 0.114 | 0.000 | 80.199 | 21.314 | 58.812 | 0.000 | [2, 2] |
| lake_indexed | observe_only | 1024 | direct | 7344 | 22.265 | 0.119 | 0.000 | 75.792 | 0.000 | 64.569 | 11.275 | [2, 3] |
| small | flush_first | 1024 | direct | 6984 | 8.856 | 0.076 | 0.000 | 53.348 | 22.715 | 32.915 | 0.000 | [1, 1] |
| small | in_band | 1024 | direct | 20325 | 11.526 | 0.076 | 0.274 | 0.004 | 0.000 | 0.000 | 0.000 | [0, 0] |
| small | observe_first | 1024 | direct | 6984 | 8.759 | 0.074 | 0.000 | 87.537 | 23.592 | 64.897 | 0.000 | [2, 2] |
| small | observe_only | 1024 | direct | 6984 | 10.999 | 0.082 | 0.000 | 106.097 | 0.000 | 70.282 | 32.317 | [2, 3] |

## Adaptive cache: five alternating baseline/candidate rounds

| cohort | hot hit rate | hot probes | hot median ms | 40 hot queries total ms |
| --- | --- | --- | --- | --- |
| 0-baseline | 0.8 | 16 | 28.095 | 1094.448 |
| 0-candidate | 1.0 | 0 | 17.091 | 757.551 |
| 1-baseline | 0.8 | 16 | 22.009 | 938.666 |
| 1-candidate | 1.0 | 0 | 18.415 | 773.263 |
| 2-baseline | 0.8 | 16 | 17.561 | 758.916 |
| 2-candidate | 1.0 | 0 | 14.252 | 621.539 |
| 3-baseline | 0.8 | 16 | 19.576 | 870.238 |
| 3-candidate | 1.0 | 0 | 16.571 | 677.378 |
| 4-baseline | 0.8 | 16 | 14.647 | 653.154 |
| 4-candidate | 1.0 | 0 | 14.048 | 566.103 |

Setup is measured separately; persistent-backend request clocks exclude it.

| variant | phase | n | request median ms | preparation median ms | connection setup median ms |
| --- | --- | --- | --- | --- | --- |
| baseline | churn | 15 | 68.827 | 12.348 | 132.651 |
| baseline | reuse_cold | 5 | 73.369 | 13.090 | 127.456 |
| baseline | reuse_warm1 | 5 | 18.266 | 0.003 | 127.456 |
| baseline | reuse_warm2 | 5 | 16.888 | 0.003 | 127.456 |
| baseline | settings_0 | 5 | 73.071 | 14.098 | 126.769 |
| baseline | settings_1 | 5 | 20.345 | 0.002 | 126.769 |
| baseline | settings_2 | 5 | 27.991 | 11.337 | 126.769 |
| baseline | settings_3 | 5 | 15.661 | 0.003 | 126.769 |
| candidate | churn | 15 | 69.087 | 12.478 | 128.854 |
| candidate | reuse_cold | 5 | 63.245 | 12.508 | 120.286 |
| candidate | reuse_warm1 | 5 | 15.422 | 0.003 | 120.286 |
| candidate | reuse_warm2 | 5 | 18.183 | 0.003 | 120.286 |
| candidate | settings_0 | 5 | 64.017 | 11.467 | 126.289 |
| candidate | settings_1 | 5 | 19.508 | 0.003 | 126.289 |
| candidate | settings_2 | 5 | 30.284 | 11.999 | 126.289 |
| candidate | settings_3 | 5 | 19.124 | 0.004 | 126.289 |
