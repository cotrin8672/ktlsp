package binaryfields;

import org.gradle.api.internal.tasks.compile.JavaCompilerArgumentsBuilder;

public final class JavaBad {
    public int wrongType() {
        return JavaCompilerArgumentsBuilder.EMPTY_SOURCE_PATH_REF_DIR;
    }
}
