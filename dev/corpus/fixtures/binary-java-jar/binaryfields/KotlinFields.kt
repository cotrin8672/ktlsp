package binaryfields

import org.gradle.api.internal.tasks.compile.JavaCompilerArgumentsBuilder

fun emptySourcePath(): String = JavaCompilerArgumentsBuilder.EMPTY_SOURCE_PATH_REF_DIR
fun compilerTableOption(): String = JavaCompilerArgumentsBuilder.USE_UNSHARED_COMPILER_TABLE_OPTION
