use proc_macro2::{TokenStream, TokenTree};
use quote::quote;
use syn::{Attribute, Data, Fields};
use synstructure::{BindingInfo, Structure, VariantInfo};

pub enum Attr {
    Name(String),
    Repr(String),
}

impl Attr {
    pub fn from_attr(attr: &Attribute) -> Option<Self> {
        if attr.path.segments[0].ident != "ipld" {
            return None;
        }
        if let TokenTree::Group(group) = attr.tokens.clone().into_iter().next().unwrap() {
            let key = if let TokenTree::Ident(key) = group.stream().into_iter().nth(0).unwrap() {
                key.to_string()
            } else {
                panic!("invalid attr");
            };
            let value =
                if let TokenTree::Literal(value) = group.stream().into_iter().nth(2).unwrap() {
                    let value = value.to_string();
                    value[1..(value.len() - 1)].to_string()
                } else {
                    panic!("invalid attr");
                };
            match key.as_str() {
                "name" => return Some(Self::Name(value)),
                "repr" => return Some(Self::Repr(value)),
                attr => panic!("Unknown attr {}", attr),
            }
        } else {
            panic!("invalid attr");
        }
    }
}

fn field_keys<'a>(bindings: &'a [BindingInfo]) -> Vec<(String, &'a BindingInfo<'a>)> {
    let mut keys: Vec<(String, &BindingInfo)> = bindings
        .iter()
        .enumerate()
        .map(|(i, binding)| {
            let field = binding.ast();
            for attr in &field.attrs {
                if let Some(Attr::Name(name)) = Attr::from_attr(attr) {
                    return (name, binding);
                }
            }
            let key = field
                .ident
                .as_ref()
                .map(|ident| ident.to_string())
                .unwrap_or_else(|| i.to_string());
            (key, binding)
        })
        .collect();
    keys.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
    keys
}

pub enum VariantRepr {
    Keyed,
    Kinded,
}

pub enum BindingRepr {
    Map,
    List,
}

impl BindingRepr {
    pub fn from_variant(variant: &VariantInfo) -> Self {
        for attr in variant.ast().attrs {
            if let Some(Attr::Repr(repr)) = Attr::from_attr(attr) {
                return match repr.as_str() {
                    "map" => Self::Map,
                    "list" => Self::List,
                    _ => panic!("unsupported repr"),
                };
            }
        }
        match variant.ast().fields {
            Fields::Named(_) => Self::Map,
            Fields::Unnamed(_) => Self::List,
            Fields::Unit => Self::List,
        }
    }

    pub fn repr(&self, bindings: &[BindingInfo]) -> TokenStream {
        let len = bindings.len() as u64;
        match self {
            Self::Map => {
                let keys = field_keys(bindings);
                let fields = keys.into_iter().map(|(key, binding)| {
                    quote! {
                        #key.write_cbor(w).await?;
                        #binding.write_cbor(w).await?;
                    }
                });
                quote! {
                    write_u64(w, 5, #len).await?;
                    #(#fields)*
                }
            }
            Self::List => {
                let fields = bindings
                    .iter()
                    .map(|binding| quote!(#binding.write_cbor(w).await?;));
                quote! {
                    write_u64(w, 4, #len).await?;
                    #(#fields)*
                }
            }
        }
    }

    pub fn parse(&self, variant: &VariantInfo) -> TokenStream {
        let len = variant.bindings().len();
        match self {
            Self::Map => {
                let keys = field_keys(variant.bindings());
                let fields = keys.into_iter().map(|(key, binding)| {
                    quote! {
                        read_key(r, #key).await?;
                        let #binding = ReadCbor::read_cbor(r).await?;
                    }
                });
                let construct = variant.construct(|_field, i| {
                    let binding = &variant.bindings()[i];
                    quote!(#binding)
                });
                quote! {
                    let len = match major {
                       0xa0..=0xb7 => major as usize - 0xa0,
                       0xb8 => read_u8(r).await? as usize,
                       _ => return Ok(None),
                    };
                    if len != #len {
                        return Err(IpldError::KeyNotFound.into());
                    }
                    #(#fields)*
                    return Ok(Some(#construct));
                }
            }
            Self::List => {
                let fields = variant
                    .bindings()
                    .iter()
                    .map(|binding| quote!(let #binding = ReadCbor::read_cbor(r).await?;));
                let construct = variant.construct(|_field, i| {
                    let binding = &variant.bindings()[i];
                    quote!(#binding)
                });
                quote! {
                    let len = match major {
                       0x80..=0x97 => major as usize - 0x80,
                       0x98 => read_u8(r).await? as usize,
                       _ => return Ok(None),
                    };
                    if len != #len {
                        return Err(IpldError::IndexNotFound.into());
                    }
                    #(#fields)*
                    return Ok(Some(#construct));
                }
            }
        }
    }
}

impl VariantRepr {
    pub fn from_structure(s: &Structure) -> Self {
        for attr in &s.ast().attrs {
            if let Some(Attr::Repr(repr)) = Attr::from_attr(attr) {
                match repr.as_str() {
                    "kinded" => {
                        return Self::Kinded;
                    }
                    "keyed" => {
                        return Self::Keyed;
                    }
                    _ => {}
                }
            }
        }
        match &s.ast().data {
            Data::Struct(_) => Self::Kinded,
            Data::Enum(_) => Self::Keyed,
            Data::Union(_) => panic!("unsupported"),
        }
    }

    pub fn repr(&self, variant: &VariantInfo) -> TokenStream {
        let binding = BindingRepr::from_variant(variant);
        let bindings = binding.repr(variant.bindings());
        match self {
            Self::Keyed => {
                let name = variant.ast().ident.to_string();
                quote! {
                    write_u64(w, 5, 1).await?;
                    #name.write_cbor(w).await?;
                    #bindings
                }
            }
            Self::Kinded => quote!(#bindings),
        }
    }

    pub fn parse(&self, variant: &VariantInfo) -> TokenStream {
        let binding = BindingRepr::from_variant(variant);
        let bindings = binding.parse(variant);
        match self {
            Self::Keyed => {
                let name = variant.ast().ident.to_string();
                quote! {
                    if key.as_str() == #name {
                        let major = read_u8(r).await?;
                        #bindings
                    }
                }
            }
            Self::Kinded => quote!(#bindings),
        }
    }
}

pub fn write_cbor(s: &Structure) -> TokenStream {
    let var_repr = VariantRepr::from_structure(s);
    let body = s.each_variant(|var| var_repr.repr(var));

    quote! {
        #[inline]
        async fn write_cbor<W: Write + Unpin + Send>(&self, w: &mut W) -> Result<()> {
            match *self {
                #body
            }
            Ok(())
        }
    }
}

pub fn read_cbor(s: &Structure) -> TokenStream {
    let var_repr = VariantRepr::from_structure(s);
    let variants: Vec<TokenStream> = s.variants().iter().map(|var| var_repr.parse(var)).collect();
    let body = match var_repr {
        VariantRepr::Keyed => {
            quote! {
                if major != 0xa1 {
                    return Ok(None);
                }
                let key = String::read_cbor(r).await?;
                #(#variants)*
                Err(IpldError::KeyNotFound.into())
            }
        }
        VariantRepr::Kinded => {
            if variants.len() > 1 {
                /*variants = variants
                .iter()
                .map(|variant| {
                    quote! {
                        if let Some(res) = (async || -> Result<Option<Self>> { #variant })().await? {
                            return Ok(Some(res));
                        }
                    }
                })
                .collect();*/
                quote! {
                    #(#variants)*
                    Err(IpldError::KeyNotFound.into())
                }
            } else {
                quote!(#(#variants)*)
            }
        }
    };

    quote! {
        #[allow(unreachable_code)]
        #[inline]
        async fn try_read_cbor<R: Read + Unpin + Send>(r: &mut R, major: u8) -> Result<Option<Self>> {
            #body
        }
    }
}
