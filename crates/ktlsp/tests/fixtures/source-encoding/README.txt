Published POM source-encoding regression fixtures

These unmodified published POMs are test data, not a production library allowlist.
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

The three Selenium 2.42.2 POMs declare Windows-1252 metadata. Their bodies happen to
be ASCII; separate synthetic tests cover every mapped non-ASCII byte and reject
the five undefined Windows-1252 bytes. This metadata support does not add a Java
source charset. Original bytes, including declarations, are retained.

Coordinates, SHA256, byte length:
org.seleniumhq.selenium:selenium-api:2.42.2
e7fabb55d15ef5618343f142330cbb22dcf0411142fd500156c413f5b6f33d16 5151
org.seleniumhq.selenium:selenium-htmlunit-driver:2.42.2
22e7d757801d4fd5b9dcfadf7059ec33a736c6ff133ef86bfbc295fff8f1fcb8 2280
org.seleniumhq.selenium:selenium-remote-driver:2.42.2
5fceb9442db62c07b77a6b247093bd9fb4fa1e7eef8ecd3a0514b058ecb87658 3583

Origin: the same existing Gradle Maven cache used by the failed source-index run.
Exact original paths, hashes, and independent strict decoding/XML parse results
are recorded in the external pom149-actual-poms.json packet.
Primary publication URLs:
https://repo.maven.apache.org/maven2/org/seleniumhq/selenium/selenium-api/2.42.2/selenium-api-2.42.2.pom
https://repo.maven.apache.org/maven2/org/seleniumhq/selenium/selenium-htmlunit-driver/2.42.2/selenium-htmlunit-driver-2.42.2.pom
https://repo.maven.apache.org/maven2/org/seleniumhq/selenium/selenium-remote-driver/2.42.2/selenium-remote-driver-2.42.2.pom

The twelve POMs listed in conditional-compiler-poms.json preserve the original
Gradle-selected bytes for six compiler-configuration failures: Guava, failureaccess,
HikariCP, JGit core/SSH, and JRuby base. The manifest records each exact coordinate,
SHA256, size, publication URL, and the selected input model SHA256. Parent POMs are
included so the regression is portable and does not depend on a user's Maven cache.
Original license declarations/notices remain intact.

published_compiler_pom_chains_share_one_verified_charset resolves all six complete
chains through the certified lookup and expects explicit/inherited UTF8, retaining
the original metadata receipts. Separate synthetic tests reject differing or
unresolved profile/execution charsets and unsafe inherited argument configurations.
