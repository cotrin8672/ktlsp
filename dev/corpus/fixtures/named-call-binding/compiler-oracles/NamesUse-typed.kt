package probe
fun namedUse(left:Class<Invocation>,right:Class<Other>,known:Other,shapes:NamedShapes,both:BothMarker) {
 val positional:Invocation=pick(left,right)
 positional.service()
 val named:Invocation=pick(second=right,first=left)
 named.service()
 val explicitDefault:Invocation=shapes.optional(required=left,ignored=known)
 explicitDefault.service()
 val omittedDefault:Invocation=shapes.optional(required=left)
 omittedDefault.service()
 known.body()
}
