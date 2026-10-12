package probe.bounds;

interface JavaLeft {
    int leftOnly();
}

interface JavaRight {
    int rightOnly();
}

interface JavaReplacement {
    int replacementOnly();
}

public class JavaBounds {
    public static <T extends JavaLeft & JavaRight> int bounded(T value) {
        return value.leftOnly() + value.rightOnly();
    }
}
