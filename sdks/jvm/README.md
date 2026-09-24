# Ailu JVM SDK

Java wrapper over `ailu-c-api` using JNA. Kotlin and Scala should consume the
same `ai.ailu.Ailu` class rather than adding a second native binding.

Set `AILU_C_API_LIB` when the native library is not on `java.library.path`.

```bash
mvn -f sdks/jvm/pom.xml test-compile dependency:build-classpath -Dmdep.outputFile=target/classpath.txt
AILU_C_API_LIB=$PWD/crates/target/debug/libailu_c_api.dylib \
  java -cp "sdks/jvm/target/classes:sdks/jvm/target/test-classes:$(cat sdks/jvm/target/classpath.txt)" \
  ai.ailu.AiluSmoke
```
