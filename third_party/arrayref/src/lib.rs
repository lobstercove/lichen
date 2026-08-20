//! Macros for taking array references to portions of arrays and slices.
//!
//! This runtime source is vendored from the previously locked `arrayref
//! 0.3.9` crate after the registry supply-chain event documented in the
//! adjacent README.
#![deny(warnings)]
#![no_std]

/// Generate an array reference to a subset of a sliceable value.
#[macro_export]
macro_rules! array_ref {
    ($arr:expr, $offset:expr, $len:expr) => {{
        {
            #[inline]
            const unsafe fn as_array<T>(slice: &[T]) -> &[T; $len] {
                &*(slice.as_ptr() as *const [_; $len])
            }
            let offset = $offset;
            let slice = &$arr[offset..offset + $len];
            #[allow(unused_unsafe)]
            unsafe {
                as_array(slice)
            }
        }
    }};
}

/// Split an array reference into contiguous array references.
#[macro_export]
macro_rules! array_refs {
    ( $arr:expr, $( $pre:expr ),* ; .. ;  $( $post:expr ),* ) => {{
        {
            use core::slice;
            #[inline]
            #[allow(unused_assignments)]
            #[allow(clippy::eval_order_dependence)]
            const unsafe fn as_arrays<T>(a: &[T]) -> ( $( &[T; $pre], )* &[T],  $( &[T; $post], )*) {
                const MIN_LEN: usize = 0usize $( .saturating_add($pre) )* $( .saturating_add($post) )*;
                assert!(MIN_LEN < usize::MAX, "Your arrays are too big, are you trying to hack yourself?!");
                let var_len = a.len() - MIN_LEN;
                assert!(a.len() >= MIN_LEN);
                let mut p = a.as_ptr();
                ( $( {
                    let aref = & *(p as *const [T; $pre]);
                    p = p.add($pre);
                    aref
                }, )* {
                    let sl = slice::from_raw_parts(p as *const T, var_len);
                    p = p.add(var_len);
                    sl
                }, $( {
                    let aref = & *(p as *const [T; $post]);
                    p = p.add($post);
                    aref
                }, )*)
            }
            let input = $arr;
            #[allow(unused_unsafe)]
            unsafe {
                as_arrays(input)
            }
        }
    }};
    ( $arr:expr, $( $len:expr ),* ) => {{
        {
            #[inline]
            #[allow(unused_assignments)]
            #[allow(clippy::eval_order_dependence)]
            const unsafe fn as_arrays<T>(a: &[T; $( $len + )* 0 ]) -> ( $( &[T; $len], )* ) {
                let mut p = a.as_ptr();
                ( $( {
                    let aref = &*(p as *const [T; $len]);
                    p = p.offset($len as isize);
                    aref
                }, )* )
            }
            let input = $arr;
            #[allow(unused_unsafe)]
            unsafe {
                as_arrays(input)
            }
        }
    }}
}

/// Split a mutable array reference into contiguous mutable array references.
#[macro_export]
macro_rules! mut_array_refs {
    ( $arr:expr, $( $pre:expr ),* ; .. ;  $( $post:expr ),* ) => {{
        {
            use core::slice;
            #[inline]
            #[allow(unused_assignments)]
            #[allow(clippy::eval_order_dependence)]
            unsafe fn as_arrays<T>(a: &mut [T]) -> ( $( &mut [T; $pre], )* &mut [T],  $( &mut [T; $post], )*) {
                const MIN_LEN: usize = 0usize $( .saturating_add($pre) )* $( .saturating_add($post) )*;
                assert!(MIN_LEN < usize::MAX, "Your arrays are too big, are you trying to hack yourself?!");
                let var_len = a.len() - MIN_LEN;
                assert!(a.len() >= MIN_LEN);
                let mut p = a.as_mut_ptr();
                ( $( {
                    let aref = &mut *(p as *mut [T; $pre]);
                    p = p.add($pre);
                    aref
                }, )* {
                    let sl = slice::from_raw_parts_mut(p as *mut T, var_len);
                    p = p.add(var_len);
                    sl
                }, $( {
                    let aref = &mut *(p as *mut [T; $post]);
                    p = p.add($post);
                    aref
                }, )*)
            }
            let input = $arr;
            #[allow(unused_unsafe)]
            unsafe {
                as_arrays(input)
            }
        }
    }};
    ( $arr:expr, $( $len:expr ),* ) => {{
        {
            #[inline]
            #[allow(unused_assignments)]
            #[allow(clippy::eval_order_dependence)]
            unsafe fn as_arrays<T>(a: &mut [T; $( $len + )* 0 ]) -> ( $( &mut [T; $len], )* ) {
                let mut p = a.as_mut_ptr();
                ( $( {
                    let aref = &mut *(p as *mut [T; $len]);
                    p = p.add($len);
                    aref
                }, )* )
            }
            let input = $arr;
            #[allow(unused_unsafe)]
            unsafe {
                as_arrays(input)
            }
        }
    }};
}

/// Generate a mutable array reference to a subset of a sliceable value.
#[macro_export]
macro_rules! array_mut_ref {
    ($arr:expr, $offset:expr, $len:expr) => {{
        {
            #[inline]
            unsafe fn as_array<T>(slice: &mut [T]) -> &mut [T; $len] {
                &mut *(slice.as_mut_ptr() as *mut [_; $len])
            }
            let offset = $offset;
            let slice = &mut $arr[offset..offset + $len];
            #[allow(unused_unsafe)]
            unsafe {
                as_array(slice)
            }
        }
    }};
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    #[test]
    fn fixed_array_macros_match_upstream_behavior() {
        let mut bytes = [0_u8, 1, 2, 3, 4, 5, 6, 7];
        let (left, right) = array_refs!(&bytes, 4, 4);
        assert_eq!(*left, [0, 1, 2, 3]);
        assert_eq!(*right, [4, 5, 6, 7]);

        let (left_mut, right_mut) = mut_array_refs!(&mut bytes, 4, 4);
        left_mut[0] = 9;
        right_mut[3] = 8;
        assert_eq!(*array_ref!(bytes, 0, 4), [9, 1, 2, 3]);
        *array_mut_ref!(bytes, 4, 4) = [7, 6, 5, 4];
        assert_eq!(bytes, [9, 1, 2, 3, 7, 6, 5, 4]);
    }
}
