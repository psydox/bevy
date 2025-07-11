use crate::field_attributes::CloneBehavior;
use crate::{
    derive_data::ReflectEnum, derive_data::StructField, field_attributes::DefaultBehavior,
    ident::ident_or_index,
};
use bevy_macro_utils::fq_std::{FQClone, FQDefault, FQOption, FQResult};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote, ToTokens};

pub(crate) struct EnumVariantOutputData {
    /// The names of each variant as a string.
    ///
    /// For example, `Some` and `None` for the `Option` enum.
    pub variant_names: Vec<String>,
    /// The pattern matching portion of each variant.
    ///
    /// For example, `Option::Some { 0: _0 }` and `Option::None {}` for the `Option` enum.
    pub variant_patterns: Vec<TokenStream>,
    /// The constructor portion of each variant.
    ///
    /// For example, `Option::Some { 0: value }` and `Option::None {}` for the `Option` enum.
    pub variant_constructors: Vec<TokenStream>,
}

#[derive(Copy, Clone)]
pub(crate) struct VariantField<'a, 'b> {
    /// The alias for the field.
    ///
    /// This should be used whenever the field needs to be referenced in a token stream.
    pub alias: &'a Ident,
    /// The name of the variant that contains the field.
    pub variant_name: &'a str,
    /// The field data.
    pub field: &'a StructField<'b>,
}

/// Trait used to control how enum variants are built.
pub(crate) trait VariantBuilder: Sized {
    /// Returns the enum data.
    fn reflect_enum(&self) -> &ReflectEnum;

    /// Returns a token stream that accesses a field of a variant as an `Option<dyn Reflect>`.
    ///
    /// The default implementation of this method will return a token stream
    /// which gets the field dynamically so as to support `dyn Enum`.
    ///
    /// # Parameters
    /// * `this`: The identifier of the enum
    /// * `field`: The field to access
    fn access_field(&self, this: &Ident, field: VariantField) -> TokenStream {
        if let Some(field_ident) = &field.field.data.ident {
            let name = field_ident.to_string();
            quote!(#this.field(#name))
        } else if let Some(field_index) = field.field.reflection_index {
            quote!(#this.field_at(#field_index))
        } else {
            quote!(::core::compile_error!(
                "internal bevy_reflect error: field should be active"
            ))
        }
    }

    /// Returns a token stream that unwraps a field of a variant as a `&dyn Reflect`
    /// (from an `Option<dyn Reflect>`).
    ///
    /// # Parameters
    /// * `field`: The field to access
    fn unwrap_field(&self, field: VariantField) -> TokenStream;

    /// Returns a token stream that constructs a field of a variant as a concrete type
    /// (from a `&dyn Reflect`).
    ///
    /// # Parameters
    /// * `field`: The field to access
    fn construct_field(&self, field: VariantField) -> TokenStream;

    /// Returns a token stream that constructs an instance of an active field.
    ///
    /// # Parameters
    /// * `this`: The identifier of the enum
    /// * `field`: The field to access
    fn on_active_field(&self, this: &Ident, field: VariantField) -> TokenStream {
        let bevy_reflect_path = self.reflect_enum().meta().bevy_reflect_path();
        let field_accessor = self.access_field(this, field);

        let alias = field.alias;
        let field_ty = field.field.reflected_type();
        let field_constructor = self.construct_field(field);

        let construction = match &field.field.attrs.default {
            DefaultBehavior::Func(path) => quote! {
                if let #FQOption::Some(#alias) = #field_accessor {
                    #field_constructor
                } else {
                    #path()
                }
            },
            DefaultBehavior::Default => quote! {
                if let #FQOption::Some(#alias) = #field_accessor {
                    #field_constructor
                } else {
                    #FQDefault::default()
                }
            },
            DefaultBehavior::Required => {
                let field_unwrapper = self.unwrap_field(field);

                quote! {{
                    // `#alias` is used by both the unwrapper and constructor
                    let #alias = #field_accessor;
                    let #alias = #field_unwrapper;
                    #field_constructor
                }}
            }
        };

        if field.field.attrs().remote.is_some() {
            quote! {
                <#field_ty as #bevy_reflect_path::ReflectRemote>::into_remote(#construction)
            }
        } else {
            construction
        }
    }

    /// Returns a token stream that constructs an instance of an ignored field.
    ///
    /// # Parameters
    /// * `field`: The field to access
    fn on_ignored_field(&self, field: VariantField) -> TokenStream {
        match &field.field.attrs.default {
            DefaultBehavior::Func(path) => quote! { #path() },
            _ => quote! { #FQDefault::default() },
        }
    }

    /// Builds the enum variant output data.
    fn build(&self, this: &Ident) -> EnumVariantOutputData {
        let variants = self.reflect_enum().variants();

        let mut variant_names = Vec::with_capacity(variants.len());
        let mut variant_patterns = Vec::with_capacity(variants.len());
        let mut variant_constructors = Vec::with_capacity(variants.len());

        for variant in variants {
            let variant_ident = &variant.data.ident;
            let variant_name = variant_ident.to_string();
            let variant_path = self.reflect_enum().get_unit(variant_ident);

            let fields = variant.fields();

            let mut field_patterns = Vec::with_capacity(fields.len());
            let mut field_constructors = Vec::with_capacity(fields.len());

            for field in fields {
                let member = ident_or_index(field.data.ident.as_ref(), field.declaration_index);
                let alias = format_ident!("_{}", member);

                let variant_field = VariantField {
                    alias: &alias,
                    variant_name: &variant_name,
                    field,
                };

                let value = if field.attrs.ignore.is_ignored() {
                    self.on_ignored_field(variant_field)
                } else {
                    self.on_active_field(this, variant_field)
                };

                field_patterns.push(quote! {
                    #member: #alias
                });

                field_constructors.push(quote! {
                    #member: #value
                });
            }

            let pattern = quote! {
                #variant_path { #( #field_patterns ),* }
            };

            let constructor = quote! {
                #variant_path {
                    #( #field_constructors ),*
                }
            };

            variant_names.push(variant_name);
            variant_patterns.push(pattern);
            variant_constructors.push(constructor);
        }

        EnumVariantOutputData {
            variant_names,
            variant_patterns,
            variant_constructors,
        }
    }
}

/// Generates the enum variant output data needed to build the `FromReflect::from_reflect` implementation.
pub(crate) struct FromReflectVariantBuilder<'a> {
    reflect_enum: &'a ReflectEnum<'a>,
}

impl<'a> FromReflectVariantBuilder<'a> {
    pub fn new(reflect_enum: &'a ReflectEnum) -> Self {
        Self { reflect_enum }
    }
}

impl<'a> VariantBuilder for FromReflectVariantBuilder<'a> {
    fn reflect_enum(&self) -> &ReflectEnum {
        self.reflect_enum
    }

    fn unwrap_field(&self, field: VariantField) -> TokenStream {
        let alias = field.alias;
        quote!(#alias?)
    }

    fn construct_field(&self, field: VariantField) -> TokenStream {
        let bevy_reflect_path = self.reflect_enum.meta().bevy_reflect_path();
        let field_ty = field.field.reflected_type();
        let alias = field.alias;

        quote! {
            <#field_ty as #bevy_reflect_path::FromReflect>::from_reflect(#alias)?
        }
    }
}

/// Generates the enum variant output data needed to build the `PartialReflect::try_apply` implementation.
pub(crate) struct TryApplyVariantBuilder<'a> {
    reflect_enum: &'a ReflectEnum<'a>,
}

impl<'a> TryApplyVariantBuilder<'a> {
    pub fn new(reflect_enum: &'a ReflectEnum) -> Self {
        Self { reflect_enum }
    }
}

impl<'a> VariantBuilder for TryApplyVariantBuilder<'a> {
    fn reflect_enum(&self) -> &ReflectEnum {
        self.reflect_enum
    }

    fn unwrap_field(&self, field: VariantField) -> TokenStream {
        let VariantField {
            alias,
            variant_name,
            field,
            ..
        } = field;

        let bevy_reflect_path = self.reflect_enum.meta().bevy_reflect_path();

        let field_name = match &field.data.ident {
            Some(ident) => format!("{ident}"),
            None => format!(".{}", field.declaration_index),
        };

        quote! {
            #alias.ok_or(#bevy_reflect_path::ApplyError::MissingEnumField {
                variant_name: ::core::convert::Into::into(#variant_name),
                field_name: ::core::convert::Into::into(#field_name)
            })?
        }
    }

    fn construct_field(&self, field: VariantField) -> TokenStream {
        let bevy_reflect_path = self.reflect_enum.meta().bevy_reflect_path();
        let alias = field.alias;
        let field_ty = field.field.reflected_type();

        quote! {
            <#field_ty as #bevy_reflect_path::FromReflect>::from_reflect(#alias)
                .ok_or(#bevy_reflect_path::ApplyError::MismatchedTypes {
                    from_type: ::core::convert::Into::into(
                        #bevy_reflect_path::DynamicTypePath::reflect_type_path(#alias)
                    ),
                    to_type: ::core::convert::Into::into(<#field_ty as #bevy_reflect_path::TypePath>::type_path())
                })?
        }
    }
}

/// Generates the enum variant output data needed to build the `Reflect::reflect_clone` implementation.
pub(crate) struct ReflectCloneVariantBuilder<'a> {
    reflect_enum: &'a ReflectEnum<'a>,
}

impl<'a> ReflectCloneVariantBuilder<'a> {
    pub fn new(reflect_enum: &'a ReflectEnum) -> Self {
        Self { reflect_enum }
    }
}

impl<'a> VariantBuilder for ReflectCloneVariantBuilder<'a> {
    fn reflect_enum(&self) -> &ReflectEnum {
        self.reflect_enum
    }

    fn access_field(&self, _ident: &Ident, field: VariantField) -> TokenStream {
        let alias = field.alias;
        quote!(#FQOption::Some(#alias))
    }

    fn unwrap_field(&self, field: VariantField) -> TokenStream {
        let alias = field.alias;
        quote!(#alias.unwrap())
    }

    fn construct_field(&self, field: VariantField) -> TokenStream {
        let bevy_reflect_path = self.reflect_enum.meta().bevy_reflect_path();
        let field_ty = field.field.reflected_type();
        let alias = field.alias;
        let alias = match &field.field.attrs.remote {
            Some(wrapper_ty) => {
                quote! {
                    <#wrapper_ty as #bevy_reflect_path::ReflectRemote>::as_wrapper(#alias)
                }
            }
            None => alias.to_token_stream(),
        };

        match &field.field.attrs.clone {
            CloneBehavior::Default => {
                quote! {
                    <#field_ty as #bevy_reflect_path::PartialReflect>::reflect_clone_and_take(#alias)?
                }
            }
            CloneBehavior::Trait => {
                quote! {
                    #FQClone::clone(#alias)
                }
            }
            CloneBehavior::Func(clone_fn) => {
                quote! {
                    #clone_fn(#alias)
                }
            }
        }
    }

    fn on_active_field(&self, _this: &Ident, field: VariantField) -> TokenStream {
        self.construct_field(field)
    }

    fn on_ignored_field(&self, field: VariantField) -> TokenStream {
        let bevy_reflect_path = self.reflect_enum.meta().bevy_reflect_path();
        let variant_name = field.variant_name;
        let alias = field.alias;

        match &field.field.attrs.clone {
            CloneBehavior::Default => {
                let field_id = field.field.field_id(bevy_reflect_path);

                quote! {
                    return #FQResult::Err(
                        #bevy_reflect_path::ReflectCloneError::FieldNotCloneable {
                            field: #field_id,
                            variant: #FQOption::Some(#bevy_reflect_path::__macro_exports::alloc_utils::Cow::Borrowed(#variant_name)),
                            container_type_path: #bevy_reflect_path::__macro_exports::alloc_utils::Cow::Borrowed(<Self as #bevy_reflect_path::TypePath>::type_path())
                        }
                    )
                }
            }
            CloneBehavior::Trait => quote! { #FQClone::clone(#alias) },
            CloneBehavior::Func(clone_fn) => quote! { #clone_fn() },
        }
    }
}
