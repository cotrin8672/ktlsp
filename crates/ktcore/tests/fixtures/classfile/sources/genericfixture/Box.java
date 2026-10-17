package genericfixture;

public class Box<T extends Item> {
    public T value;
    public T get() { return value; }
    public T[] array() { return null; }
    public <U extends Item> U same(U value) { return value; }
    public <T extends Other> T shadow(T value) { return value; }
    public java.util.List<? extends T> producers;
    public java.util.List<? super T> consumers;
    public java.util.List<?> stars;
    public java.util.Map.Entry<String, T> entry;
}
