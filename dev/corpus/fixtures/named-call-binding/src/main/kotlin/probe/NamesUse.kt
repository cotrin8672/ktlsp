package probe
fun namedUse(left:Class<Invocation>,right:Class<Other>,known:Other,shapes:NamedShapes,both:BothMarker) {
 pick(left,right).service()
 pick(second=right,first=left).service()
 shapes.optional(required=left,ignored=known).service()
 shapes.optional(required=left).service()
 known.body()
}
