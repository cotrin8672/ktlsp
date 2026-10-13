package binaryfields;

import org.gradle.api.internal.tasks.compile.JavaCompilerArgumentsBuilder;

public final class JavaFields {
    public String emptySourcePath() {
        return JavaCompilerArgumentsBuilder.EMPTY_SOURCE_PATH_REF_DIR;
    }

    public String compilerTableOption() {
        return JavaCompilerArgumentsBuilder.USE_UNSHARED_COMPILER_TABLE_OPTION;
    }
}
