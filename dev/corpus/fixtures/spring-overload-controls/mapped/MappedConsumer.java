package mapped;
public final class MappedConsumer {
    public <T> MappedResult consume(java.util.List<? extends T> values) { return new MappedResult(); }
    public MappedResult consume(MappedResult value) { return value; }
}
