# Known defects of the typed run

False findings of `unused-class-members` with type-aware analysis on: a live class member is reported although it is
read. Measured on the seeded workspace of the dead-code benchmark, where the typed run has a precision of 81% against
100% for the untyped run. The figures and cases are those reported when the type-aware code was moved out of skif;
no test of this crate reproduces them yet, so each needs a failing test before it is fixed.

The member is read, and reported unused, when the read goes:

1. **Through an interface the class satisfies without `implements`.** `show(item: Labelled)` called with
   `new Badge()`: the read of `item.label` reaches the member of `Labelled`, and `Badge` is not credited because it
   never names the interface.
2. **Through a generic constraint.** `heaviest<T extends HasWeight>([new Parcel()])`: the read of `T`'s member
   reaches the constraint `HasWeight`, not `Parcel`.
3. **Through a key of literal type.** `frame[axis]` with `axis: "height"`: the element access by a variable of a
   literal type is not taken as a read of `height`.

In all three the checker answers for the declared type of the receiver, and the facts hold no edge from that type to
the classes structurally assignable to it. The untyped analysis credits these reads by name, which is why it reports
none of them.
