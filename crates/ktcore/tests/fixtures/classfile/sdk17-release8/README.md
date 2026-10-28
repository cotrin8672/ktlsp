# Selected Java 8 SDK API signatures

The headers below are unchanged entries from Oracle JDK 17.0.7's `lib/ct.sym`.
The archive SHA-256 is `4b93c3b65cd8a1abb8a294269ce0074ec31a17f1b0b73b5c2fe49922200e7fd3`.
The existing `SDK-LICENSE` and `SDK-COPYRIGHT` apply.

| Entry | Bytes | SHA-256 |
| --- | --- | --- |
| `AbstractMap.sig` | 1345 | `746024a7e06a85c9a0ceae3eaf8b88d5e71d1fbc9e5034e982b0f25c872542a4` |
| `AbstractMap$SimpleEntry.sig` | 818 | `d2ab35e8fb6129ff75928ae909b28b9f32c95dc2d7867b8f90f7f00732c38a82` |
| `ProcessBuilder.sig` | 1296 | `b0d4d411182a939a028aaa22a2c41bd5d6a483a676fb55b2b44f9c9754d966f9` |
| `ProcessBuilder$Redirect.sig` | 548 | `a8717d6d0ce384de934e261a90aba3aaa677a3acc8141ce69d9fd18bc30dfc53` |
| `ProcessBuilder$Redirect$Type.sig` | 563 | `e1547ab84d00d35624a3d9b91d63c8bbdb4820a5cfaa9b69cc73f32cea94e31a` |
| `ConcurrentHashMap.sig` | 8019 | `db835b848644c19f0b4f52dc226e5935cadb230aa3a66e97e76aab799c8b425a` |
| `ConcurrentHashMap$KeySetView.sig` | 1094 | `7a8791d47748bb2e9bf70c91e2b36a764d9032b3656ed6af2a0c73c21de3aa3e` |
| `ConcurrentHashMap$CollectionView.sig` | 1060 | `dd532b89949b949a8b9178a002bd72f6e7f83542c5347c72c1a823c3a211e69d` |

The AbstractMap headers use `87/java.base/java/util/`.
The ProcessBuilder headers use `87/java.base/java/lang/`, except Type, which uses
`879A/java.base/java/lang/`. They preserve the two-level public static owner chain.
The ConcurrentHashMap headers use `8/java.base/java/util/concurrent/` and preserve
the public KeySetView's generic package-private static CollectionView superclass.

The SDK regression uses their reciprocal InnerClasses rows, generic Map.Entry
hierarchy and two real constructors. These API headers contain no executable Code.
