package pluginfixture;

public final class JavaUse {
    public static Object serializerFromJava() { return Model.Companion.serializer(); }
    public static String lombokFromJava() { return new Person("Java").getName(); }
    public static NeedsLabel noArgFromBinary() { return new NeedsLabel(); }
}
