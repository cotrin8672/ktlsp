package binarygenerics

import genericfixture.Child
import genericfixture.Holder
import genericfixture.Item
import genericfixture.Other

fun useGenerics(child: Child, item: Item, other: Other) {
    child.get().ping()
    child.value.ping()
    child.array()[0].ping()
    child.same(item).ping()
    child.shadow(other).pong()
    Holder.ITEM.get().ping()
}
