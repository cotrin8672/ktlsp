import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.ImportTree;
import com.sun.source.tree.IdentifierTree;
import com.sun.source.tree.MemberSelectTree;
import com.sun.source.tree.MethodInvocationTree;
import com.sun.source.tree.NewClassTree;
import com.sun.source.util.TreePath;
import com.sun.source.util.TreePathScanner;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.ExecutableElement;
import javax.lang.model.element.TypeElement;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.TypeKind;
import javax.tools.StandardLocation;
import com.sun.source.util.JavacTask;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;
import com.sun.source.util.Trees;
import java.io.Reader;
import java.io.StringWriter;
import java.nio.charset.Charset;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Objects;
import java.util.Properties;
import javax.tools.Diagnostic;
import javax.tools.JavaCompiler;
import javax.tools.JavaFileObject;
import javax.tools.StandardJavaFileManager;
import javax.tools.ToolProvider;

public final class JavacDiagnostics {
    private static String require(Properties p, String key) {
        return Objects.requireNonNull(p.getProperty(key), "Missing property " + key);
    }

    private static String sha(Path path) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        try (var stream = Files.newInputStream(path)) {
            byte[] buffer = new byte[65536];
            for (int n; (n = stream.read(buffer)) >= 0;) digest.update(buffer, 0, n);
        }
        return java.util.HexFormat.of().formatHex(digest.digest());
    }

    private static Map<String, Object> runtime() {
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("javaHome", System.getProperty("java.home"));
        out.put("javaVersion", System.getProperty("java.version"));
        out.put("javaRuntimeVersion", System.getProperty("java.runtime.version"));
        out.put("fileEncoding", System.getProperty("file.encoding"));
        out.put("nativeEncoding", System.getProperty("sun.jnu.encoding"));
        out.put("defaultCharset", Charset.defaultCharset().name());
        out.put("defaultLocale", Locale.getDefault().toLanguageTag());
        return out;
    }

    private static List<Object> sourceStates(List<Path> sources) throws Exception {
        List<Object> out = new ArrayList<>();
        for (Path source : sources) {
            Map<String, Object> row = new LinkedHashMap<>();
            row.put("path", source.toString());
            row.put("uri", source.toUri().toString());
            row.put("bytes", Files.size(source));
            row.put("sha256", sha(source));
            out.add(row);
        }
        return out;
    }

    private static Map<String, Object> diagnostic(Diagnostic<? extends JavaFileObject> d,
            Locale locale, int ordinal) {
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("ordinal", ordinal);
        out.put("kind", d.getKind().name());
        out.put("code", d.getCode());
        out.put("sourceUri", d.getSource() == null ? null : d.getSource().toUri().toString());
        out.put("sourceName", d.getSource() == null ? null : d.getSource().getName());
        out.put("line", d.getLineNumber());
        out.put("column", d.getColumnNumber());
        out.put("position", d.getPosition());
        out.put("start", d.getStartPosition());
        out.put("end", d.getEndPosition());
        out.put("message", d.getMessage(locale));
        out.put("messageLocale", locale.toLanguageTag());
        return out;
    }

    private static List<Object> supportedOptions(JavaCompiler compiler,
            StandardJavaFileManager manager, List<String> options) {
        List<Object> out = new ArrayList<>();
        for (int i = 0; i < options.size();) {
            String option = options.get(i);
            int arity = compiler.isSupportedOption(option);
            String owner = "JavaCompiler";
            if (arity < 0) {
                arity = manager.isSupportedOption(option);
                owner = "StandardJavaFileManager";
            }
            if (arity < 0 || option.startsWith("@") || option.startsWith("-J"))
                throw new IllegalArgumentException("Unsupported/opaque API option at " + i + ": " + option);
            if (i + arity >= options.size())
                throw new IllegalArgumentException("Missing argument for API option: " + option);
            Map<String, Object> row = new LinkedHashMap<>();
            row.put("position", i);
            row.put("option", option);
            row.put("arity", arity);
            row.put("recognizedBy", owner);
            out.add(row);
            i += 1 + arity;
        }
        return out;
    }

    private static String quote(String value) {
        StringBuilder out = new StringBuilder("\"");
        for (int i = 0; i < value.length(); i++) {
            char ch = value.charAt(i);
            if (ch == '"' || ch == '\\') out.append('\\').append(ch);
            else if (ch < 32 || ch > 126) out.append(String.format(Locale.ROOT, "\\u%04x", (int) ch));
            else out.append(ch);
        }
        return out.append('"').toString();
    }

    private static String json(Object value) {
        if (value == null) return "null";
        if (value instanceof String) return quote((String) value);
        if (value instanceof Boolean || value instanceof Number) return value.toString();
        if (value instanceof Map<?, ?> map) {
            List<String> rows = new ArrayList<>();
            for (var entry : map.entrySet())
                rows.add(quote((String) entry.getKey()) + ":" + json(entry.getValue()));
            return "{" + String.join(",", rows) + "}";
        }
        if (value instanceof Iterable<?> list) {
            List<String> rows = new ArrayList<>();
            for (Object row : list) rows.add(json(row));
            return "[" + String.join(",", rows) + "]";
        }
        throw new IllegalArgumentException("Unserializable receipt value " + value.getClass());
    }

    // Observe the original task: a second parse would not bind these compiler diagnostics.
    private static void observeImports(JavaCompiler.CompilationTask task, List<Object> imports,
            List<Object> parsedUnits, List<Object> platformCalls, StandardJavaFileManager manager,
            List<Map<String, Object>> diagnostics) {
        if (!(task instanceof JavacTask javac))
            throw new IllegalStateException("Selected SDK does not expose public JavacTask");
        Trees trees = Trees.instance(javac);
        javac.addTaskListener(new TaskListener() {
            @Override public void started(TaskEvent event) {}
            @Override public void finished(TaskEvent event) {
                if (event.getKind() == TaskEvent.Kind.ANALYZE)
                    observePlatformCalls(javac, trees, manager, event, platformCalls, diagnostics);
                if (event.getKind() != TaskEvent.Kind.PARSE) return;
                CompilationUnitTree unit = Objects.requireNonNull(event.getCompilationUnit());
                String uri = unit.getSourceFile().toUri().toString();
                parsedUnits.add(uri);
                for (ImportTree imported : unit.getImports()) {
                    Map<String, Object> row = new LinkedHashMap<>();
                    row.put("sourceUri", uri);
                    row.put("static", imported.isStatic());
                    row.put("name", imported.getQualifiedIdentifier().toString());
                    row.put("start", trees.getSourcePositions().getStartPosition(unit, imported));
                    row.put("end", trees.getSourcePositions().getEndPosition(unit, imported));
                    row.put("qualifiedStart", trees.getSourcePositions().getStartPosition(unit,
                        imported.getQualifiedIdentifier()));
                    row.put("qualifiedEnd", trees.getSourcePositions().getEndPosition(unit,
                        imported.getQualifiedIdentifier()));
                    imports.add(row);
                }
            }
        });
    }


    // Same-task attribution; only the selected SDK's complete java.base API is eligible.
    private static void observePlatformCalls(JavacTask javac, Trees trees,
            StandardJavaFileManager manager, TaskEvent event, List<Object> calls,
            List<Map<String, Object>> diagnostics) {
        CompilationUnitTree unit = Objects.requireNonNull(event.getCompilationUnit());
        String uri = unit.getSourceFile().toUri().toString();
        if (diagnostics.stream().noneMatch(d -> "ERROR".equals(d.get("kind"))
                && "compiler.err.cant.resolve.location.args".equals(d.get("code"))
                && uri.equals(d.get("sourceUri")))) return;
        TreePath owner = trees.getPath(event.getTypeElement());
        if (owner == null) return;
        new TreePathScanner<Void, Void>() {
            @Override public Void visitMethodInvocation(MethodInvocationTree call, Void unused) {
                if (call.getMethodSelect() instanceof MemberSelectTree select
                        && call.getArguments().isEmpty() && call.getTypeArguments().isEmpty()) {
                    var expression = select.getExpression();
                    long selectStart = trees.getSourcePositions().getStartPosition(unit, select);
                    long selectEnd = trees.getSourcePositions().getEndPosition(unit, select);
                    long receiverEnd = trees.getSourcePositions().getEndPosition(unit, expression);
                    Map<String, Object> diagnostic = diagnostics.stream().filter(d ->
                        "ERROR".equals(d.get("kind"))
                        && "compiler.err.cant.resolve.location.args".equals(d.get("code"))
                        && uri.equals(d.get("sourceUri")) && Long.valueOf(selectStart).equals(d.get("start"))
                        && Long.valueOf(selectEnd).equals(d.get("end"))
                        && Long.valueOf(receiverEnd).equals(d.get("position"))).findFirst().orElse(null);
                    if (diagnostic == null) return super.visitMethodInvocation(call, unused);
                    TreePath receiverPath = new TreePath(new TreePath(getCurrentPath(), select), expression);
                    var element = trees.getElement(receiverPath);
                    boolean variable = expression instanceof IdentifierTree && element != null
                        && (element.getKind() == ElementKind.LOCAL_VARIABLE
                            || element.getKind() == ElementKind.PARAMETER);
                    boolean construction = expression instanceof NewClassTree created
                        && created.getClassBody() == null && created.getTypeArguments().isEmpty()
                        && element instanceof ExecutableElement
                        && element.getKind() == ElementKind.CONSTRUCTOR;
                    var mirror = trees.getTypeMirror(receiverPath);
                    if ((variable || construction) && mirror != null && mirror.getKind() == TypeKind.DECLARED
                            && mirror instanceof DeclaredType declared && declared.getTypeArguments().isEmpty()
                            && declared.asElement() instanceof TypeElement type && type.getTypeParameters().isEmpty()
                            && javac.getElements().getModuleOf(type).getQualifiedName().contentEquals("java.base")
                            && trees.getPath(type) == null
                            && type.equals(javac.getElements().getTypeElement(javac.getElements().getModuleOf(type),
                                type.getQualifiedName()))) {
                        try {
                            String binary = javac.getElements().getBinaryName(type).toString();
                            var location = manager.getLocationForModule(StandardLocation.SYSTEM_MODULES, "java.base");
                            JavaFileObject origin = manager.getJavaFileForInput(location, binary, JavaFileObject.Kind.CLASS);
                            String expected = "jrt:/java.base/" + binary.replace('.', '/') + ".class";
                            if (origin != null && "java.base".equals(manager.inferModuleName(location))
                                    && origin.toUri().toString().equals(expected)) {
                                long start = trees.getSourcePositions().getStartPosition(unit, expression);
                                long end = trees.getSourcePositions().getEndPosition(unit, expression);
                                String source = unit.getSourceFile().getCharContent(true).toString();
                                if (start >= 0 && end > start && end <= source.length()) {
                                    Map<String, Object> row = new LinkedHashMap<>();
                                    row.put("sourceUri", uri);
                                    row.put("diagnosticOrdinal", diagnostic.get("ordinal"));
                                    row.put("receiverText", source.substring((int) start, (int) end));
                                    row.put("receiverKind", variable ? "variable" : "construction");
                                    row.put("receiverBinaryName", binary);
                                    row.put("originUri", expected);
                                    row.put("method", select.getIdentifier().toString());
                                    row.put("receiverStart", start);
                                    row.put("receiverEnd", end);
                                    row.put("selectStart", trees.getSourcePositions().getStartPosition(unit, select));
                                    row.put("selectEnd", trees.getSourcePositions().getEndPosition(unit, select));
                                    row.put("callStart", trees.getSourcePositions().getStartPosition(unit, call));
                                    row.put("callEnd", trees.getSourcePositions().getEndPosition(unit, call));
                                    calls.add(row);
                                }
                            }
                        } catch (Exception unavailable) {
                            // Unavailable origin is not absence authority.
                        }
                    }
                }
                return super.visitMethodInvocation(call, unused);
            }
        }.scan(owner, null);
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 2) throw new IllegalArgumentException("spec.properties receipt.json");
        Path spec = Path.of(args[0]).toAbsolutePath().normalize();
        Path output = Path.of(args[1]).toAbsolutePath().normalize();
        if (Files.exists(output)) throw new IllegalStateException("Receipt already exists " + output);
        Properties p = new Properties();
        try (Reader reader = Files.newBufferedReader(spec, StandardCharsets.UTF_8)) { p.load(reader); }
        Locale locale = Locale.forLanguageTag(require(p, "effective.locale"));
        Charset charset = Charset.forName(require(p, "effective.charset"));
        String encodingNull = require(p, "gradle.configuredEncodingNull");
        if (!encodingNull.equals("true") && !encodingNull.equals("false"))
            throw new IllegalArgumentException("Invalid configured encoding flag");
        boolean configuredEncodingNull = Boolean.parseBoolean(encodingNull);
        List<String> options = new ArrayList<>();
        for (int i = 0; i < Integer.parseInt(require(p, "option.count")); i++)
            options.add(require(p, "option." + i));
        List<Path> sources = new ArrayList<>();
        for (int i = 0; i < Integer.parseInt(require(p, "source.count")); i++)
            sources.add(Path.of(require(p, "source." + i + ".path")).toAbsolutePath().normalize());
        List<Object> before = sourceStates(sources);
        List<Map<String, Object>> diagnostics = new ArrayList<>();
        StringWriter compilerOutput = new StringWriter();
        List<Object> imports = new ArrayList<>();
        List<Object> parsedUnits = new ArrayList<>();
        List<Object> platformCalls = new ArrayList<>();
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("component", "ktlsp selected-SDK public Javac API");
        out.put("stage", require(p, "stage"));
        out.put("inputSpecSha256", sha(spec));
        out.put("runtime", runtime());
        out.put("gradleFileEncoding", require(p, "gradle.fileEncoding"));
        out.put("gradleConfiguredEncodingNull", configuredEncodingNull);
        out.put("gradleConfiguredEncoding", configuredEncodingNull ? null : charset.name());
        out.put("gradleEffectiveDefaultCharset", require(p, "effective.charset"));
        out.put("gradleEffectiveDefaultLocale", require(p, "effective.locale"));
        out.put("explicitFileManagerCharset", charset.name());
        out.put("explicitDiagnosticLocale", locale.toLanguageTag());
        out.put("options", options);
        out.put("sourcesBefore", before);
        boolean completed = false;
        boolean success = false;
        Throwable failure = null;
        try {
            if (!Path.of(System.getProperty("java.home")).toRealPath().equals(
                    Path.of(require(p, "expected.javaHome")).toRealPath()))
                throw new IllegalStateException("Selected SDK home differs");
            if (!System.getProperty("java.version").equals(require(p, "expected.javaVersion")))
                throw new IllegalStateException("Selected SDK version differs");
            if (!System.getProperty("java.runtime.version").equals(require(p, "expected.javaRuntimeVersion")))
                throw new IllegalStateException("Selected SDK runtime version differs");
            for (int i = 0; i < sources.size(); i++)
                if (!sha(sources.get(i)).equals(require(p, "source." + i + ".sha256")))
                    throw new IllegalStateException("Source hash differs " + sources.get(i));
            JavaCompiler compiler = Objects.requireNonNull(ToolProvider.getSystemJavaCompiler(), "JDK compiler absent");
            out.put("compilerClass", compiler.getClass().getName());
            try (StandardJavaFileManager manager = compiler.getStandardFileManager(
                    d -> diagnostics.add(diagnostic(d, locale, diagnostics.size())), locale, charset)) {
                out.put("fileManagerClass", manager.getClass().getName());
                out.put("supportedOptions", supportedOptions(compiler, manager, options));
                var units = manager.getJavaFileObjectsFromPaths(sources);
                JavaCompiler.CompilationTask task = compiler.getTask(compilerOutput, manager,
                    d -> diagnostics.add(diagnostic(d, locale, diagnostics.size())), options, null, units);
                task.setLocale(locale);
                observeImports(task, imports, parsedUnits, platformCalls, manager, diagnostics);
                success = Boolean.TRUE.equals(task.call());
                completed = true;
            }
            List<Object> after = sourceStates(sources);
            out.put("sourcesAfter", after);
            if (!before.equals(after)) throw new IllegalStateException("Sources changed during compilation");
        } catch (Throwable error) {
            failure = error;
            Map<String, Object> thrown = new LinkedHashMap<>();
            thrown.put("type", error.getClass().getName());
            thrown.put("message", error.getMessage());
            StringWriter trace = new StringWriter();
            error.printStackTrace(new java.io.PrintWriter(trace));
            thrown.put("stack", trace.toString());
            out.put("failure", thrown);
        }
        Map<String, Integer> kinds = new LinkedHashMap<>();
        for (Map<String, Object> d : diagnostics)
            kinds.merge((String) d.get("kind"), 1, Integer::sum);
        out.putIfAbsent("failure", null);
        out.put("compileCompleted", completed && failure == null);
        out.put("success", success && failure == null);
        out.put("diagnostics", diagnostics);
        out.put("imports", imports);
        out.put("parsedUnits", parsedUnits);
        out.put("platformCalls", platformCalls);
        out.put("countsByKind", kinds);
        out.put("compilerOutput", compilerOutput.toString());
        out.put("wholeApiAuthority", false);
        out.put("missingTypesAuthoritative", false);
        out.put("producerClosure", false);
        Files.writeString(output, json(out) + "\n", StandardCharsets.UTF_8, StandardOpenOption.CREATE_NEW);
        System.exit(failure != null ? 2 : success ? 0 : 1);
    }
}
