package arrayfixture;

public final class Factory {
    public String name() { return "fixture"; }
    public Item[][] rows() { return new Item[0][0]; }
    public int[] counts() { return new int[0]; }
    public <T> T[] generic() { return null; }
    @Deprecated public Item[] annotated() { return null; }
    public static Item[] staticRows() { return null; }
    public Item[] withArgument(int ignored) { return null; }
    protected Item[] hidden() { return null; }
}
