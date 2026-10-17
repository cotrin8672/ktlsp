package shadowfixture;

public class Container {
    public static class NestedList<T> extends java.util.AbstractList<T> {
        public T get(int index) { return null; }
        public int size() { return 0; }
    }
}
