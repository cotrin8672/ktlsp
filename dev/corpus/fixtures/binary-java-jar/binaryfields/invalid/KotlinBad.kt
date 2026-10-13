package binaryfields

import org.gradle.api.internal.tasks.compile.JavaCompilerArgumentsBuilder

fun wrongType(): Int = JavaCompilerArgumentsBuilder.EMPTY_SOURCE_PATH_REF_DIR
