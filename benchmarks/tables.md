# Benchmark tables

Cell: one-worker median, then speedup at 8 and 32 workers against one worker of the same run and operation total.
Runtimes ran at different times under different outside load, so compare curves within a runtime.

## Environments

- `bun-cipher-single-tcache`: bun 1.4.0, cpus 64, load 12.34, 2026-09-24T13:22:46.068Z
- `bun-sqlcipher-sqlcipher-tcache`: bun 1.4.0, cpus 64, load 20.44, 2026-09-24T22:34:38.598Z
- `bun-threadsafe`: bun 1.4.0, cpus 64, load 9.61, 2026-09-24T13:03:41.500Z
- `chrome-cipher`: chrome Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessC, cpus 64, load 16.46, 2026-09-24T15:44:56.282Z
- `chrome-single-tcache`: chrome Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessC, cpus 64, load 9.97, 2026-09-24T16:20:05.227Z
- `chrome-sqlcipher-sqlcipher-tcache`: chrome Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessC, cpus 64, load 16.74, 2026-09-24T22:54:38.448Z
- `chrome-threadsafe`: chrome Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessC, cpus 64, load 4.72, 2026-09-24T14:26:53.059Z
- `chrome-threadsafe-nomutex`: chrome Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessC, cpus 64, load 24.86, 2026-09-24T15:36:57.083Z
- `chrome-threadsafe-sahpool-writes`: chrome Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessC, cpus 64, load 2.17, 2026-09-24T15:26:18.262Z
- `firefox-cipher`: firefox Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:156.0) Gecko/20100101 Firefox/156.0, cpus 64, load 14.49, 2026-09-24T17:55:57.002Z
- `firefox-single`: firefox Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:156.0) Gecko/20100101 Firefox/156.0, cpus 64, load 6.86, 2026-09-24T18:48:56.399Z
- `firefox-sqlcipher-sqlcipher-tcache`: firefox Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:156.0) Gecko/20100101 Firefox/156.0, cpus 64, load 9.77, 2026-09-24T23:12:52.286Z
- `firefox-tcache`: firefox Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:156.0) Gecko/20100101 Firefox/156.0, cpus 64, load 10.78, 2026-09-24T18:54:50.925Z
- `firefox-threadsafe-a`: firefox Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:156.0) Gecko/20100101 Firefox/156.0, cpus 64, load 11.45, 2026-09-24T16:45:52.479Z
- `firefox-threadsafe-b`: firefox Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:156.0) Gecko/20100101 Firefox/156.0, cpus 64, load 16.05, 2026-09-24T17:16:28.733Z
- `node`: node v24.13.1, cpus 64, load 4.89, 2026-09-24T12:04:32.284Z
- `node-sqlcipher`: node v24.13.1, cpus 64, load 18.31, 2026-09-25T06:35:11.392Z
- `node-sqlcipher-coldscan`: node v24.13.1, cpus 64, load 32.49, 2026-09-24T21:29:45.531Z
- `node-sqlcipher-tcache`: node v24.13.1, cpus 64, load 42.77, 2026-09-24T22:31:01.957Z
- `node-tcache`: node v24.13.1, cpus 64, load 8.42, 2026-09-24T14:16:02.202Z
- `node-threadsafe-nomemstatus`: node v24.13.1, cpus 64, load 17.76, 2026-09-24T20:49:14.384Z
- `webkit-threadsafe`: webkit Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version, cpus 8, load 10.83, 2026-09-24T19:14:54.353Z
- `webkit-threadsafe-b`: webkit Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version, cpus 8, load 17.36, 2026-09-24T20:27:49.475Z

## memvfs, one shared database

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 222 ms, x0.8, x0.6 | 662 ms, x1.2, x1.0 | 253 ms, x0.7, x0.7 | 216 ms, x0.8, x0.5 | 210 ms, x0.6, x0.5
point in one read transaction | 127 ms, x4.9, x12.2 | 213 ms, x5.6, x15.4 | 130 ms, x5.5, x14.1 | 122 ms, x6.2, x16.4 | 119 ms, x5.1, x13.5
range | 275 ms, x6.1, x18.8 | 331 ms, x7.1, x18.6 | 282 ms, x7.3, x17.3 | 263 ms, x6.3, x18.2 | 257 ms, x6.5, x18.6
range in one read transaction | 280 ms, x6.9, x19.4 | 321 ms, x6.8, x18.8 | 278 ms, x7.0, x18.1 | 262 ms, x6.6, x18.0 | 251 ms, x6.7, x17.7
scan | 1155 ms, x0.4, x0.3 | 1965 ms, x0.4, x0.3 | 1071 ms, x0.4, x0.3 | 1074 ms, x0.4, x0.3 | 1140 ms, x0.4, x0.3
sort | 538 ms, x6.6, x8.9 | 881 ms, x4.9, x11.9 | 630 ms, x6.9, x11.9 | 537 ms, x6.4, x7.4 | 617 ms, x6.9, x13.4
sort in one read transaction | 529 ms, x6.8, x9.3 | 840 ms, x6.7, x11.5 | 632 ms, x6.9, x12.8 | 543 ms, x6.7, x6.9 | 620 ms, x6.6, x13.1
fts | 182 ms, x3.4, x1.6 | 160 ms, x1.3, x1.0 | 160 ms, x2.6, x1.8 | 191 ms, x2.2, x1.7 | 152 ms, x2.5, x1.8
fts in one read transaction | 178 ms, x3.0, x2.1 | 164 ms, x1.3, x1.1 | 157 ms, x2.8, x1.8 | 190 ms, x2.6, x1.7 | 148 ms, x2.7, x1.8
insert | 219 ms, x0.4, x0.1 | 422 ms, x0.5, x0.2 | 228 ms, x0.4, x0.1 | 234 ms, x0.4, x0.1 | 226 ms, x0.5, x0.1
mix | 205 ms, x0.3, x0.1 | 422 ms, x0.2, x0.1 | 212 ms, x0.3, x0.2 | 227 ms, x0.3, x0.1 | 209 ms, x0.4, x0.1

## memvfs, one database per worker

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 225 ms, x1.3, x0.8 | 454 ms, x1.5, x1.1 | 241 ms, x1.3, x0.9 | 236 ms, x1.3, x0.9 | 227 ms, x1.1, x1.0
point in one read transaction | 121 ms, x6.2, x14.2 | 203 ms, x6.7, x16.4 | 137 ms, x3.6, x8.6 | 125 ms, x5.8, x12.7 | 124 ms, x6.3, x16.2
range | 269 ms, x5.7, x17.6 | 307 ms, x6.7, x18.5 | 282 ms, x7.2, x16.3 | 271 ms, x6.7, x17.8 | 258 ms, x6.4, x16.2
range in one read transaction | 266 ms, x5.7, x18.0 | 305 ms, x6.3, x19.0 | 287 ms, x7.4, x16.9 | 266 ms, x6.5, x18.8 | 255 ms, x7.0, x14.8
scan | 1176 ms, x0.5, x0.3 | 2015 ms, x0.4, x0.3 | 1386 ms, x0.5, x0.4 | 1306 ms, x0.5, x0.3 | 1551 ms, x0.6, x0.5
sort | 544 ms, x6.7, x9.8 | 881 ms, x6.9, x11.0 | 647 ms, x6.8, x11.0 | 549 ms, x6.6, x9.9 | 636 ms, x6.7, x11.6
sort in one read transaction | 537 ms, x6.1, x10.2 | 855 ms, x6.9, x11.1 | 633 ms, x6.9, x11.0 | 546 ms, x6.0, x10.6 | 637 ms, x6.4, x12.5
fts | 192 ms, x2.5, x2.2 | 163 ms, x1.3, x1.0 | 173 ms, x2.4, x1.9 | 181 ms, x3.2, x2.0 | 156 ms, x2.7, x1.8
fts in one read transaction | 187 ms, x4.2, x2.2 | 166 ms, x1.5, x1.1 | 158 ms, x1.1, x1.1 | 179 ms, x2.6, x1.6 | 151 ms, x2.6, x1.8
insert | 233 ms, x1.2, x0.9 | 436 ms, x2.3, x1.3 | n/a | 239 ms, x1.4, x1.0 | 235 ms, x1.3, x0.9
mix | 221 ms, x0.8, x0.5 | 436 ms, x1.1, x0.7 | n/a | 236 ms, x0.7, x0.5 | 215 ms, x0.8, x0.5

## private :memory: database per worker

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 150 ms, x2.2, x1.6 | 261 ms, x2.6, x2.0 | n/a | 156 ms, x2.4, x1.7 | 144 ms, x1.7, x1.5
point in one read transaction | 115 ms, x6.1, x14.8 | 207 ms, x7.0, x17.7 | n/a | 123 ms, x5.8, x15.4 | 117 ms, x6.5, x14.6
range | 267 ms, x6.6, x19.0 | 319 ms, x6.8, x18.5 | n/a | 282 ms, x7.1, x19.5 | 244 ms, x6.4, x16.8
range in one read transaction | 266 ms, x5.5, x18.7 | 304 ms, x6.9, x19.4 | n/a | 285 ms, x7.2, x20.2 | 246 ms, x6.9, x18.5
scan | 1178 ms, x0.5, x0.3 | 2071 ms, x0.4, x0.3 | n/a | 1357 ms, x0.4, x0.3 | 1195 ms, x0.4, x0.4
sort | 530 ms, x6.6, x9.8 | 871 ms, x6.7, x10.4 | n/a | 564 ms, x6.6, x9.4 | 621 ms, x6.8, x12.3
sort in one read transaction | 525 ms, x6.7, x9.9 | 857 ms, x6.7, x9.9 | n/a | 539 ms, x5.8, x9.2 | 618 ms, x6.9, x12.3
fts | 184 ms, x3.0, x2.1 | 161 ms, x1.4, x1.0 | n/a | 190 ms, x2.6, x1.7 | 147 ms, x2.4, x1.9
fts in one read transaction | 184 ms, x4.8, x1.8 | 167 ms, x1.4, x1.1 | n/a | 189 ms, x2.7, x1.6 | 144 ms, x3.1, x1.8
insert | 188 ms, x2.7, x1.6 | 372 ms, x1.9, x1.6 | n/a | 205 ms, x2.1, x1.0 | 201 ms, x1.9, x1.5
mix | 122 ms, x1.3, x0.6 | 234 ms, x0.8, x0.7 | n/a | 133 ms, x0.8, x1.0 | 118 ms, x0.7, x0.6

## OPFS sahpool, one pool per worker (flush-bound control)

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 909 ms, x5.3, x4.9 | 10076 ms, x1.4, x1.0 | n/a | n/a | n/a
point in one read transaction | 120 ms, x6.2, x15.1 | 218 ms, x6.7, x17.6 | n/a | n/a | n/a
range | 280 ms, x5.8, x18.2 | 532 ms, x4.1, x2.6 | n/a | n/a | n/a
range in one read transaction | 266 ms, x6.3, x18.2 | 317 ms, x6.8, x18.3 | n/a | n/a | n/a
scan | 1170 ms, x0.5, x0.3 | 1852 ms, x0.4, x0.3 | n/a | n/a | n/a
sort | 554 ms, x6.2, x10.6 | 1053 ms, x6.5, x7.8 | n/a | n/a | n/a
sort in one read transaction | 542 ms, x6.2, x11.5 | 841 ms, x6.7, x10.6 | n/a | n/a | n/a
fts | 206 ms, x4.6, x2.2 | 380 ms, x2.6, x2.2 | n/a | n/a | n/a
fts in one read transaction | 181 ms, x4.0, x2.1 | 166 ms, x1.3, x1.0 | n/a | n/a | n/a
insert | 4955 ms, x4.0, x8.9 | 9775 ms, x7.4?, x20.3 | n/a | n/a | n/a
mix | 22473 ms, x5.1?, x37.8 | 36742 ms, x5.0?, x3.6? | n/a | n/a | n/a

## SQLite3MC, one shared encrypted database

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 5133 ms, x7.0, x7.8 | 8012 ms, x7.0, x7.3 | n/a | 5300 ms, x6.2, x7.5 | 6536 ms, x7.2, x8.6
point in one read transaction | 134 ms, x5.2, x14.3 | 214 ms, x6.2, x16.0 | n/a | 127 ms, x5.7, x16.5 | 122 ms, x5.4, x10.8
range | 379 ms, x6.7, x18.9 | 461 ms, x6.9, x18.1 | n/a | 355 ms, x6.1, x17.5 | 365 ms, x6.8, x18.5
range in one read transaction | 286 ms, x6.9, x19.6 | 305 ms, x6.8, x19.1 | n/a | 261 ms, x6.4, x18.1 | 247 ms, x5.9, x18.5
scan | 1177 ms, x0.4, x0.3 | 1966 ms, x0.4, x0.3 | n/a | 1061 ms, x0.3, x0.2 | 1046 ms, x0.4, x0.3
sort | 648 ms, x6.6, x10.7 | 931 ms, x6.3, x11.2 | n/a | 618 ms, x4.3, x8.9 | 684 ms, x6.6, x13.1
sort in one read transaction | 556 ms, x6.5, x10.9 | 837 ms, x7.0, x11.0 | n/a | 745 ms, x5.6, x8.0 | 583 ms, x6.5, x12.2
fts | 279 ms, x3.3, x2.5 | 258 ms, x1.5, x1.5 | n/a | 366 ms, x4.3, x2.7 | 218 ms, x3.5, x2.5
fts in one read transaction | 199 ms, x2.7, x1.9 | 157 ms, x1.3, x0.9 | n/a | 181 ms, x2.0, x1.6 | 141 ms, x2.5, x1.7
insert | 1243 ms, x0.8, x0.4 | 1874 ms, x0.8, x0.6 | n/a | 1271 ms, x0.8, x0.4 | 1430 ms, x0.9, x0.4
mix | 4663 ms, x0.2, x0.1 | 6871 ms, x0.2, x0.1 | n/a | 4761 ms, x0.2, x0.1 | 5462 ms, x0.2, x0.1

## SQLite3MC, one encrypted database per worker

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 5187 ms, x7.2, x14.4 | 7951 ms, x7.5, x13.8 | n/a | 5096 ms, x7.2, x13.7 | 6335 ms, x7.4, x18.6
point in one read transaction | 126 ms, x5.2, x14.4 | 210 ms, x6.7, x16.7 | n/a | 119 ms, x6.4, x12.7 | 118 ms, x5.2, x16.6
range | 377 ms, x6.8, x18.1 | 458 ms, x7.0, x16.7 | n/a | 348 ms, x6.4, x17.4 | 359 ms, x6.5, x18.7
range in one read transaction | 283 ms, x6.7, x19.4 | 327 ms, x6.2, x19.7 | n/a | 254 ms, x6.2, x17.9 | 245 ms, x6.9, x18.6
scan | 1312 ms, x0.5, x0.3 | 2434 ms, x0.5, x0.4 | n/a | 1143 ms, x0.4, x0.3 | 1123 ms, x0.4, x0.3
sort | 629 ms, x6.7, x10.3 | 957 ms, x7.1, x11.2 | n/a | 610 ms, x6.2, x8.6 | 669 ms, x6.7, x12.1
sort in one read transaction | 567 ms, x6.9, x9.9 | 825 ms, x5.5, x5.9 | n/a | 569 ms, x6.4, x10.3 | 582 ms, x6.7, x11.4
fts | 264 ms, x3.3, x2.4 | 330 ms, x2.1, x1.9 | n/a | 260 ms, x3.6, x2.4 | 223 ms, x4.5, x2.7
fts in one read transaction | 197 ms, x2.8, x1.7 | 160 ms, x1.2, x0.9 | n/a | 173 ms, x3.1, x1.5 | 142 ms, x2.9, x1.7
insert | 1226 ms, x5.2, x4.1 | 1887 ms, x5.9, x4.9 | n/a | 1233 ms, x5.1, x3.8 | 1438 ms, x6.0, x5.0
mix | 4591 ms, x6.5, x8.0 | 6662 ms, x6.8, x9.6 | n/a | 4761 ms, x6.5, x7.8 | 5605 ms, x7.1, x11.0

## memvfs shared, NOMUTEX connections

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 228 ms, x0.7, x0.6 | 349 ms, x0.8, x0.5 | n/a | 479 ms, x1.1, x1.1 | 188 ms, x0.5, x0.5
point in one read transaction | 116 ms, x5.7, x12.8 | 132 ms, x6.7, x17.3 | n/a | 116 ms, x5.2, x12.6 | 105 ms, x6.1, x16.9
range | 293 ms, x6.5, x19.9 | 297 ms, x6.8, x17.8 | n/a | 273 ms, x4.4, x9.6 | 265 ms, x6.9, x19.4
range in one read transaction | 277 ms, x6.9, x18.6 | 301 ms, x7.1, x18.9 | n/a | 272 ms, x6.5, x17.9 | 265 ms, x6.9, x20.6
scan | 1184 ms, x0.4, x0.3 | 1649 ms, x0.3, x0.3 | n/a | 2683 ms, x0.8, x0.7 | 1159 ms, x0.4, x0.4
sort | 572 ms, x6.2, x10.8 | 812 ms, x6.8, x10.5 | n/a | 529 ms, x5.4, x8.3 | 659 ms, x7.2, x13.8
sort in one read transaction | 576 ms, x6.8, x10.8 | 810 ms, x6.9, x11.1 | n/a | 552 ms, x7.2, x9.0 | 624 ms, x6.9, x13.3
fts | 195 ms, x2.7, x1.8 | 162 ms, x1.3, x1.0 | n/a | 207 ms, x2.9, x1.9 | 155 ms, x3.1, x1.8
fts in one read transaction | 192 ms, x2.9, x1.7 | 152 ms, x1.4, x1.0 | n/a | 177 ms, x3.1, x1.8 | 147 ms, x2.3, x1.9
insert | 211 ms, x0.5, x0.1 | 307 ms, x0.5, x0.1 | n/a | 228 ms, x0.5, x0.1 | 204 ms, x0.4, x0.1
mix | 223 ms, x0.3, x0.2 | 356 ms, x0.3, x0.1 | n/a | 221 ms, x0.3, x0.1 | 189 ms, x0.2, x0.1

## private :memory: per worker, NOMUTEX connections

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 145 ms, x2.0, x1.5 | 189 ms, x1.6, x1.3 | n/a | 150 ms, x2.4, x1.7 | 132 ms, x1.4, x1.3
point in one read transaction | 109 ms, x6.2, x15.4 | 129 ms, x6.6, x14.5 | n/a | 113 ms, x6.2, x15.7 | 103 ms, x5.6, x16.4
range | 276 ms, x6.8, x19.7 | 306 ms, x7.2, x19.1 | n/a | 273 ms, x4.6, x6.6 | 251 ms, x6.8, x18.9
range in one read transaction | 274 ms, x6.9, x18.9 | 306 ms, x7.1, x16.7 | n/a | 420 ms, x6.3, x9.5 | 266 ms, x6.8, x20.5
scan | 1238 ms, x0.4, x0.3 | 2124 ms, x0.4, x0.3 | n/a | 1518 ms, x0.6, x0.4 | 1166 ms, x0.4, x0.4
sort | 554 ms, x6.1, x9.5 | 877 ms, x7.3, x11.0 | n/a | 535 ms, x6.3, x10.0 | 633 ms, x6.9, x13.1
sort in one read transaction | 586 ms, x6.7, x10.4 | 817 ms, x6.5, x10.3 | n/a | 545 ms, x6.5, x10.2 | 637 ms, x7.1, x13.3
fts | 199 ms, x1.7, x1.4 | 158 ms, x1.3, x1.0 | n/a | 179 ms, x2.3, x1.7 | 151 ms, x2.6, x1.7
fts in one read transaction | 198 ms, x2.2, x1.8 | 164 ms, x1.2, x1.0 | n/a | 181 ms, x2.4, x1.6 | 149 ms, x2.6, x1.8
insert | 186 ms, x1.9, x1.2 | 254 ms, x1.3, x1.1 | n/a | 188 ms, x1.9, x1.2 | 175 ms, x1.8, x1.3
mix | 120 ms, x0.8, x0.6 | 184 ms, x0.6, x0.5 | n/a | 120 ms, x0.9, x0.5 | 112 ms, x0.7, x0.6

## one serving worker over postMessage, clients on x

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 5851 ms, x3.1, x2.5 | 33136 ms, x4.1, x4.1 | 59331 ms, x5.0, x3.7 | 4598 ms, x3.4, x3.4 | 3927 ms, x2.7, x2.8
range | 391 ms, x1.2, x1.2 | 1136 ms, x2.7, x2.6 | 1616 ms, x3.6, x4.1 | 408 ms, x1.4, x1.4 | 377 ms, x1.3, x1.4

## private :memory:, per-thread allocation caches

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 157 ms, x2.3, x1.6 | 275 ms, x2.4, x2.1 | n/a | 155 ms, x2.2, x1.8 | 142 ms, x1.6, x1.4
point in one read transaction | 119 ms, x6.5, x15.8 | 219 ms, x7.5, x15.0 | n/a | 119 ms, x6.5, x18.5 | 113 ms, x5.2, x15.6
range | 285 ms, x6.8, x20.0 | 331 ms, x7.1, x20.0 | n/a | 261 ms, x7.0, x18.8 | 234 ms, x6.4, x17.1
range in one read transaction | 290 ms, x7.1, x20.2 | 329 ms, x7.6, x17.2 | n/a | 259 ms, x7.1, x18.7 | 233 ms, x6.3, x17.4
scan | 1884 ms, x0.6, x0.4 | 3494 ms, x0.5, x0.5 | n/a | 1658 ms, x0.5, x0.4 | 1316 ms, x0.5, x0.4
sort | 581 ms, x6.8, x18.8 | 868 ms, x7.0, x21.0 | n/a | 526 ms, x6.5, x17.1 | 630 ms, x6.8, x19.2
sort in one read transaction | 592 ms, x7.0, x19.9 | 876 ms, x7.0, x21.3 | n/a | 517 ms, x6.8, x17.1 | 621 ms, x7.1, x19.7
fts | 196 ms, x5.2, x4.4 | 156 ms, x2.5, x1.9 | n/a | 189 ms, x5.4, x4.4 | 143 ms, x4.3, x3.1
fts in one read transaction | 200 ms, x5.7, x5.0 | 160 ms, x2.7, x2.0 | n/a | 178 ms, x5.5, x4.1 | 140 ms, x4.4, x3.1
insert | 190 ms, x4.4, x3.5 | 364 ms, x4.2, x3.0 | n/a | 192 ms, x4.8, x3.7 | 176 ms, x2.7, x2.3
mix | 122 ms, x1.4, x1.1 | 202 ms, x1.4, x1.0 | n/a | 114 ms, x1.6, x1.2 | 105 ms, x0.8, x0.7

## memvfs per worker, per-thread allocation caches

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 231 ms, x1.3, x1.1 | 477 ms, x1.6, x1.3 | n/a | 219 ms, x1.6, x1.0 | 214 ms, x1.1, x1.0
point in one read transaction | 126 ms, x5.5, x15.1 | 222 ms, x7.4, x17.5 | n/a | 121 ms, x6.4, x10.8 | 118 ms, x6.6, x14.7
range | 286 ms, x6.9, x20.2 | 306 ms, x6.8, x19.0 | n/a | 255 ms, x5.9, x13.6 | 241 ms, x6.4, x17.1
range in one read transaction | 283 ms, x6.3, x20.2 | 310 ms, x6.0, x18.3 | n/a | 252 ms, x5.7, x14.0 | 235 ms, x6.3, x18.1
scan | 1646 ms, x0.5, x0.4 | 3016 ms, x0.5, x0.4 | n/a | 1324 ms, x0.4, x0.3 | 1258 ms, x0.5, x0.4
sort | 575 ms, x6.8, x19.0 | 1187 ms, x4.9, x7.9 | n/a | 515 ms, x6.5, x17.5 | 621 ms, x7.0, x20.0
sort in one read transaction | 561 ms, x6.7, x18.8 | 1240 ms, x7.6, x19.0 | n/a | 512 ms, x6.6, x16.4 | 616 ms, x7.0, x20.0
fts | 187 ms, x5.0, x4.4 | 234 ms, x3.2, x2.7 | n/a | 175 ms, x4.7, x4.0 | 147 ms, x4.1, x3.1
fts in one read transaction | 184 ms, x5.1, x4.4 | 154 ms, x2.4, x1.9 | n/a | 176 ms, x4.7, x4.3 | 140 ms, x4.5, x3.2
insert | 219 ms, x4.5, x4.3 | 451 ms, x6.3, x5.3 | n/a | 223 ms, x5.4, x4.7 | 198 ms, x5.1, x4.8
mix | 213 ms, x1.4, x0.9 | 436 ms, x1.6, x1.2 | n/a | 212 ms, x1.4, x1.0 | 179 ms, x1.3, x0.9

## memvfs shared, per-thread allocation caches

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | 233 ms, x0.8, x0.6 | 467 ms, x0.9, x0.7 | n/a | 215 ms, x0.8, x0.5 | 197 ms, x0.6, x0.5
point in one read transaction | 128 ms, x6.5, x12.3 | 227 ms, x6.3, x18.7 | n/a | 116 ms, x6.1, x15.0 | 122 ms, x6.6, x16.8
range | 287 ms, x6.8, x14.9 | 316 ms, x5.8, x18.3 | n/a | 260 ms, x6.9, x13.9 | 239 ms, x6.6, x17.5
range in one read transaction | 282 ms, x6.2, x14.6 | 319 ms, x6.7, x19.5 | n/a | 256 ms, x5.8, x17.4 | 236 ms, x7.0, x16.8
scan | 1264 ms, x0.5, x0.3 | 2205 ms, x0.4, x0.3 | n/a | 1113 ms, x0.4, x0.2 | 1054 ms, x0.4, x0.3
sort | 544 ms, x6.8, x18.2 | 851 ms, x7.2, x19.2 | n/a | 499 ms, x6.7, x17.4 | 628 ms, x6.7, x20.1
sort in one read transaction | 550 ms, x7.1, x18.9 | 838 ms, x7.1, x20.0 | n/a | 499 ms, x6.9, x16.4 | 622 ms, x6.8, x19.6
fts | 187 ms, x5.6, x4.2 | 159 ms, x2.4, x1.8 | n/a | 183 ms, x5.2, x3.6 | 147 ms, x4.6, x3.0
fts in one read transaction | 190 ms, x5.6, x4.4 | 164 ms, x2.8, x2.1 | n/a | 170 ms, x4.8, x3.9 | 140 ms, x4.5, x3.2
insert | 221 ms, x0.4, x0.1 | 410 ms, x0.5, x0.2 | n/a | 218 ms, x0.4, x0.1 | 194 ms, x0.4, x0.1
mix | 194 ms, x0.2, x0.1 | 410 ms, x0.3, x0.1 | n/a | 186 ms, x0.2, x0.1 | 173 ms, x0.3, x0.1

## private :memory:, memstatus off

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | n/a | n/a | n/a | 155 ms, x2.7, x1.9 | n/a
point in one read transaction | n/a | n/a | n/a | 127 ms, x5.5, x13.8 | n/a
range | n/a | n/a | n/a | 281 ms, x7.1, x18.5 | n/a
range in one read transaction | n/a | n/a | n/a | 281 ms, x7.3, x19.5 | n/a
scan | n/a | n/a | n/a | 1355 ms, x0.7, x0.3 | n/a
sort | n/a | n/a | n/a | 537 ms, x6.6, x13.1 | n/a
sort in one read transaction | n/a | n/a | n/a | 542 ms, x6.5, x12.8 | n/a
fts | n/a | n/a | n/a | 175 ms, x4.4, x1.7 | n/a
fts in one read transaction | n/a | n/a | n/a | 174 ms, x4.3, x1.6 | n/a
insert | n/a | n/a | n/a | 194 ms, x2.8, x1.0 | n/a
mix | n/a | n/a | n/a | 119 ms, x1.0, x0.4 | n/a

## memvfs shared, memstatus off

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
point | n/a | n/a | n/a | 233 ms, x0.7, x0.6 | n/a
point in one read transaction | n/a | n/a | n/a | 127 ms, x5.4, x12.4 | n/a
range | n/a | n/a | n/a | 280 ms, x6.6, x19.1 | n/a
range in one read transaction | n/a | n/a | n/a | 276 ms, x7.6, x17.4 | n/a
scan | n/a | n/a | n/a | 1231 ms, x0.6, x0.2 | n/a
sort | n/a | n/a | n/a | 637 ms, x7.8, x15.2 | n/a
sort in one read transaction | n/a | n/a | n/a | 530 ms, x6.5, x13.3 | n/a
fts | n/a | n/a | n/a | 200 ms, x4.3, x1.8 | n/a
fts in one read transaction | n/a | n/a | n/a | 196 ms, x4.4, x1.9 | n/a
insert | n/a | n/a | n/a | 236 ms, x0.4, x0.1 | n/a
mix | n/a | n/a | n/a | 240 ms, x0.3, x0.2 | n/a

## SQLCipher, one shared keyed database

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
keyed_open | 21633 ms, x1.0, x0.3 | 21758 ms, x1.2, x0.3 | n/a | 21340 ms, x1.0, x0.3 | 23642 ms, x0.8, x0.3
cold_scan | 3979 ms, x7.6, x14.1 | 2349 ms, x7.3, x9.3 | n/a | 2326 ms, x6.9, x13.5 | 2270 ms, x7.3, x9.6

## SQLCipher build, unencrypted shared database

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
keyed_open | 3 ms, x0.5, x0.3 | 3 ms, x0.2, x0.2 | n/a | 2 ms, x0.4, x0.3 | 1 ms, x0.3, x0.2
cold_scan | 781 ms, x5.8, x11.3 | 489 ms, x5.9, x14.2 | n/a | 436 ms, x6.3, x17.2 | 402 ms, x7.0, x17.3

## SQLCipher, per-thread allocation caches

workload | chrome | firefox | webkit | node | bun
--- | --- | --- | --- | --- | ---
keyed_open | 20881 ms, x7.8, x28.4 | 21388 ms, x7.7, x27.1 | n/a | 21180 ms, x8.0, x22.4 | 23081 ms, x7.9, x19.1
cold_scan | 2298 ms, x7.4, x21.5 | 2334 ms, x7.1, x19.9 | n/a | 2252 ms, x7.7, x19.0 | 2281 ms, x7.7, x19.7

`?` marks a point whose q3 exceeds twice its q1.
