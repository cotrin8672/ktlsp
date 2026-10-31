//! Checked JVMS declaration annotations; no framework interpretation or default expansion.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryAnnotation {
    pub runtime_visible: bool,
    pub annotation: Annotation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub descriptor: Arc<str>,
    pub elements: Vec<AnnotationElement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationElement {
    pub name: Arc<str>,
    pub value: AnnotationValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnnotationValue {
    Byte(i8), Char(u16), DoubleBits(u64), FloatBits(u32), Int(i32), Long(i64),
    Short(i16), Boolean(bool), String(Arc<str>),
    /// Exact code units when a Java string contains unpaired surrogates.
    StringUtf16(Arc<[u16]>),
    Enum { descriptor: Arc<str>, name: Arc<str> },
    Class(Arc<str>), Annotation(Box<Annotation>), Array(Vec<AnnotationValue>),
}

impl Annotation {
    /// Conservative heap weight, including shared text at each use, for bounded metadata caches.
    pub fn heap_size(&self) -> usize {
        text_size(&self.descriptor) + self.elements.capacity() * std::mem::size_of::<AnnotationElement>()
            + self.elements.iter().map(|element|text_size(&element.name)+element.value.heap_size()).sum::<usize>()
    }
}
impl AnnotationValue {
    fn heap_size(&self) -> usize {
        match self {
            Self::String(value) | Self::Class(value) => text_size(value),
            Self::StringUtf16(value) => value.len()*std::mem::size_of::<u16>()+2*std::mem::size_of::<usize>(),
            Self::Enum {descriptor,name} => text_size(descriptor)+text_size(name),
            Self::Annotation(value) => std::mem::size_of::<Annotation>()+value.heap_size(),
            Self::Array(values) => values.capacity()*std::mem::size_of::<Self>()+values.iter().map(Self::heap_size).sum::<usize>(),
            _ => 0,
        }
    }
}

fn text_size(value: &str) -> usize {
    value.len() + 2 * std::mem::size_of::<usize>()
}

fn node(strings: &mut Strings<'_, '_>, depth: usize) -> Result<()> {
    ensure!(depth <= 64, "annotation nesting limit");
    strings.remaining_annotation_nodes = strings.remaining_annotation_nodes.checked_sub(1)
        .context("annotation node limit")?;
    Ok(())
}

fn utf8(strings: &mut Strings<'_, '_>, input: &mut &[u8]) -> Result<Arc<str>> {
    let value = strings.get(cpool::Index::new(take_u16(input)?)?)?;
    strings.charge(value.len())?;
    Ok(value)
}

fn string_value(strings: &mut Strings<'_, '_>, input: &mut &[u8]) -> Result<AnnotationValue> {
    let index=cpool::Index::<cpool::Utf8<'_>>::new(take_u16(input)?)?;
    let raw=strings.pool.retrieve(index)?;
    strings.charge(raw.len())?;
    if let Some(value)=strings.values.get(&index.as_u16()) {
        return Ok(AnnotationValue::String(value.clone()));
    }
    if let Some(value)=strings.utf16_values.get(&index.as_u16()) {
        return Ok(AnnotationValue::StringUtf16(value.clone()));
    }
    if raw.chars().all(|ch|ch.is_ok()) {
        return Ok(AnnotationValue::String(strings.get(index)?));
    }
    // Java strings can contain lone surrogates; Unicode scalar strings cannot preserve them.
    let mut units=Vec::new();
    for ch in raw.chars() {
        match ch {
            Ok(ch)=>units.extend_from_slice(ch.encode_utf16(&mut [0;2])),
            Err(surrogate)=>units.push(u16::try_from(surrogate).context("invalid UTF16 code unit")?),
        }
    }
    let units:Arc<[u16]>=units.into();
    strings.utf16_values.insert(index.as_u16(),units.clone());
    Ok(AnnotationValue::StringUtf16(units))
}

fn descriptor(strings: &mut Strings<'_, '_>, input: &mut &[u8], object: bool) -> Result<Arc<str>> {
    let value = utf8(strings,input)?;
    if !object && value.as_ref()=="V" { return Ok(value); }
    let encoded = noak::MString::from(value.as_ref());
    let shape = TypeDescriptor::parse(&encoded)?;
    validate_type_name(&shape)?;
    ensure!(!object || (shape.dimensions==0 && matches!(shape.base,BaseType::Object(_))),
        "annotation requires object descriptor");
    Ok(value)
}

fn annotation(strings: &mut Strings<'_, '_>, input: &mut &[u8], depth: usize) -> Result<Annotation> {
    node(strings,depth)?;
    let descriptor=descriptor(strings,input,true)?;
    let count=usize::from(take_u16(input)?);
    ensure!(count<=input.len()/5,"truncated annotation elements");
    let mut elements=Vec::with_capacity(count);
    let mut names=HashSet::new();
    for _ in 0..count {
        let name=utf8(strings,input)?;
        ensure!(unqualified_name(&name) && !name.contains(['<','>']),"invalid annotation element name");
        ensure!(names.insert(name.clone()),"duplicate annotation element");
        elements.push(AnnotationElement {name,value:value(strings,input,depth+1)?});
    }
    Ok(Annotation {descriptor,elements})
}

fn value(strings: &mut Strings<'_, '_>, input: &mut &[u8], depth: usize) -> Result<AnnotationValue> {
    node(strings,depth)?;
    let (&tag,rest)=input.split_first().context("truncated annotation value")?;
    *input=rest;
    use AnnotationValue::*;
    Ok(match tag {
        b'B'|b'C'|b'I'|b'S'|b'Z' => {
            let number=strings.pool.get(cpool::Index::<cpool::Integer>::new(take_u16(input)?)?)?.value;
            match tag {
                b'B'=>Byte(number.try_into().context("annotation byte range")?),
                b'C'=>Char(number.try_into().context("annotation char range")?),
                b'S'=>Short(number.try_into().context("annotation short range")?),
                b'Z'=> {ensure!(matches!(number,0|1),"annotation boolean range");Boolean(number==1)},
                _=>Int(number),
            }
        },
        b'J'=>Long(strings.pool.get(cpool::Index::<cpool::Long>::new(take_u16(input)?)?)?.value),
        b'F'=>FloatBits(strings.pool.get(cpool::Index::<cpool::Float>::new(take_u16(input)?)?)?.value.to_bits()),
        b'D'=>DoubleBits(strings.pool.get(cpool::Index::<cpool::Double>::new(take_u16(input)?)?)?.value.to_bits()),
        b's'=>string_value(strings,input)?,
        b'e'=> {
            let descriptor=descriptor(strings,input,true)?;
            let name=utf8(strings,input)?;
            ensure!(unqualified_name(&name) && !name.contains(['<','>']),"invalid annotation enum name");
            Enum {descriptor,name}
        },
        b'c'=>Class(descriptor(strings,input,false)?),
        b'@'=>Annotation(Box::new(annotation(strings,input,depth+1)?)),
        b'['=> {
            let count=usize::from(take_u16(input)?);
            ensure!(count<=input.len()/3,"truncated annotation array");
            let mut values=Vec::with_capacity(count);
            for _ in 0..count { values.push(value(strings,input,depth+1)?); }
            Array(values)
        },
        _=>anyhow::bail!("invalid annotation value tag"),
    })
}

pub(super) fn decode(strings: &mut Strings<'_, '_>, mut input: &[u8], runtime_visible: bool) -> Result<Vec<BinaryAnnotation>> {
    let count=usize::from(take_u16(&mut input)?);
    ensure!(count<=input.len()/4,"truncated annotations");
    let mut values=Vec::with_capacity(count);
    for _ in 0..count { values.push(BinaryAnnotation {runtime_visible,annotation:annotation(strings,&mut input,0)?}); }
    ensure!(input.is_empty(),"trailing annotation bytes");
    Ok(values)
}
