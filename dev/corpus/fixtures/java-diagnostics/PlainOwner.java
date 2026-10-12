package app;

public class PlainOwner {
    public int accept(String value) { return 1; }
    public int accept(int value, int count) { return value + count; }
    public void wait(String value) {}
    public <T> T identity(T value) { return value; }
    public int many(int... values) { return values.length; }
}
