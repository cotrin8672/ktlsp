package defaultfixture;

public interface Overlay {
    int MARKER = 1;
    default <T> T tooltip(T value) { return value; }
    static int fromStatic() { return MARKER; }
    void writeSafe(Thread.State unsupported);
    private void helper() {}
}
