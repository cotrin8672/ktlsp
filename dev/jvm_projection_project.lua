-- Disposable Kotlin/JVM facade fixture independently compiled with Kotlin 1.9.23 and javac --release 17.
-- nvim -l dev/jvm_projection_project.lua <project>
local root = assert(arg[1], 'missing disposable project directory')
local function write(path, text)
  local full = root .. '/' .. path
  assert(not vim.uv.fs_stat(full), 'fixture file already exists: ' .. full)
  vim.fn.mkdir(vim.fs.dirname(full), 'p')
  vim.fn.writefile(vim.split(text, '\n', { plain = true, trimempty = true }), full)
end

write('KotlinExtensions.kt', [[@file:JvmName("KotlinExtensions")
package interop

class Payload {
    fun label(): String = "payload"
}

class Call<T>(val value: T) {
    fun regular(): String = "ordinary"
}

@JvmName("awaitNullable")
suspend fun <T : Any> Call<T?>.await(): T? = value]])

write('KotlinUse.kt', [[package interop

suspend fun kotlinUse(call: Call<Payload?>): Payload? {
    call.regular()
    return call.await()
}]])

write('JavaUse.java', [[package interop;
import kotlin.coroutines.Continuation;

public class JavaUse {
    public static Object run(Call<Payload> call, Continuation<? super Payload> continuation) {
        call.regular();
        return KotlinExtensions.awaitNullable(call, continuation);
    }

    public static void main(String[] args) {
        Payload payload = new Payload();
        Object result = run(new Call<Payload>(payload), null);
        if (result != payload) throw new AssertionError("wrong target");
    }
}]])

write('Decoy.java', [[package interop;
import kotlin.coroutines.Continuation;

public class Decoy {
    public static Object awaitNullable(Call<Payload> call, Continuation<? super Payload> continuation) {
        return "wrong target";
    }
}]])
