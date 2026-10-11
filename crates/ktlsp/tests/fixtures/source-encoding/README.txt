Published POM source-encoding regression fixtures

These three unmodified published POMs are test data, not a production library allowlist.
They establish the existing Commons Compress 1.26.0 -> Commons Parent 66 -> Apache 31
static parent/property chain. The reader also has synthetic unrelated-coordinate tests.

Coordinates, SHA256, byte length:
org.apache.commons:commons-compress:1.26.0
c1c9681f88d9421730c4da6176492bbaa54052a2326df9e40bc34f4c405227af 22362
org.apache.commons:commons-parent:66
48fd6dc846e56b1f408660d163e75300f9e384bb63be482a8082a21d72a8db9c 77001
org.apache:apache:31
555d0c9eaa69c042aff924927b9381e8f8174136d355eead445224452e6291cc 23531

Origin: existing local Gradle Maven cache; exact paths and before/after hashes are
recorded in the external encoding packet's pom-fixtures-provenance.json.
The POMs contain their Apache License 2.0 notices, which remain unchanged.
Primary publication URLs:
https://repo.maven.apache.org/maven2/org/apache/commons/commons-compress/1.26.0/commons-compress-1.26.0.pom
https://repo.maven.apache.org/maven2/org/apache/commons/commons-parent/66/commons-parent-66.pom
https://repo.maven.apache.org/maven2/org/apache/apache/31/apache-31.pom

The source_encoding tests resolve this chain through a cached-file lookup;
they do not download metadata, activate profiles, execute plugins, or load user classes.
