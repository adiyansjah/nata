#[cfg(kani)]
mod perf2c_proofs {
    use crate::decimal::batch_magnitudes;
    use crate::rejected_decimal::product_precision;
    #[kani::proof]
    fn perf2c_precision() {
        let (a, b, p): (u8, u8, u8) = kani::any();
        let got = product_precision(a, b, p);
        let spec = a > 0
            && a <= 38
            && b > 0
            && b <= 38
            && p > 0
            && p <= 38
            && u32::from(a) + u32::from(b) <= u32::from(p);
        assert_eq!(got, spec);
        kani::cover!(got);
        kani::cover!(!got);
    }
    #[kani::proof]
    #[kani::unwind(3)]
    fn perf2c_scan() {
        let a: [i128; 2] = kani::any();
        let b: [i128; 2] = kani::any();
        let av: [bool; 2] = kani::any();
        let bv: [bool; 2] = kani::any();
        let selected: [bool; 2] = kani::any();
        let (la, lb): (u128, u128) = kani::any();
        kani::assume(la > 0 && lb > 0);
        let mut sel = [0; 2];
        let mut n = 0;
        let mut spec = true;
        for i in 0..2 {
            if selected[i] {
                sel[n] = i as u32;
                n += 1;
                if av[i] && bv[i] {
                    spec &= a[i].unsigned_abs() < la && b[i].unsigned_abs() < lb;
                }
            }
        }
        let got = batch_magnitudes(&a, &av, &b, &bv, &sel[..n], la, lb);
        assert_eq!(got, spec);
        kani::cover!(got);
        kani::cover!(!got);
    }
    // All signed i16 operands; multiply exactly in i32, decimal precisions 1..4.
    #[kani::proof]
    #[kani::unwind(10)]
    fn perf2c_reduced_product() {
        let (x, y): (i16, i16) = kani::any();
        let (a, b): (u8, u8) = kani::any();
        kani::assume(a > 0 && a <= 4 && b > 0 && b <= 4);
        let la = 10i32.pow(u32::from(a));
        let lb = 10i32.pow(u32::from(b));
        kani::assume(i32::from(x).abs() < la && i32::from(y).abs() < lb);
        let v = i32::from(x) * i32::from(y);
        assert!(v.abs() < 10i32.pow(u32::from(a) + u32::from(b)));
        kani::cover!(x < 0 && y > 0);
        kani::cover!(x > 0 && y < 0);
    }
    #[kani::proof]
    #[kani::unwind(10)]
    fn perf2c_full_product_heavy() {
        let (x, y): (i128, i128) = kani::any();
        let (a, b, p): (u8, u8, u8) = kani::any();
        kani::assume(product_precision(a, b, p));
        kani::assume(x.unsigned_abs() < 10u128.pow(u32::from(a)));
        kani::assume(y.unsigned_abs() < 10u128.pow(u32::from(b)));
        let v = x.checked_mul(y).unwrap();
        assert!(v.unsigned_abs() < 10u128.pow(u32::from(p)));
    }

    #[kani::proof]
    #[kani::unwind(10)]
    fn perf2c_aligned_precision() {
        let (pa,pb,p,da,db):(u8,u8,u8,u8,u8)=kani::any();
        kani::assume(da<=38 && db<=38);
        let la=10i128.pow(u32::from(da)); let lb=10i128.pow(u32::from(db));
        let got=crate::decimal::aligned_precision(pa,pb,la,lb,p);
        let spec=pa>0 && pa<=38 && pb>0 && pb<=38 && p>0 && p<=38
            && u16::from(pa)+u16::from(da)<u16::from(p)
            && u16::from(pb)+u16::from(db)<u16::from(p);
        assert_eq!(got,spec); kani::cover!(got); kani::cover!(!got);
    }
    #[kani::proof]
    #[kani::unwind(10)]
    fn perf2c_reduced_aligned() {
        let (x,y):(i16,i16)=kani::any();
        let (pa,pb,da,db,p):(u8,u8,u8,u8,u8)=kani::any();
        kani::assume(pa>0 && pb>0 && pa<=4 && pb<=4 && da<=3 && db<=3 && p<=8);
        kani::assume(u16::from(pa)+u16::from(da)<u16::from(p));
        kani::assume(u16::from(pb)+u16::from(db)<u16::from(p));
        kani::assume(i32::from(x).abs()<10i32.pow(u32::from(pa)));
        kani::assume(i32::from(y).abs()<10i32.pow(u32::from(pb)));
        let a=i32::from(x)*10i32.pow(u32::from(da));
        let b=i32::from(y)*10i32.pow(u32::from(db));
        assert!((a+b).abs()<10i32.pow(u32::from(p)));
        assert!((a-b).abs()<10i32.pow(u32::from(p)));
        kani::cover!(x<0 && y>0); kani::cover!(x>0 && y<0);
    }
    #[kani::proof]
    fn perf2c_sum_envelope() {
        let (current,bound):(u128,u128)=kani::any(); let n:u64=kani::any();
        let got=crate::sum_admission::batch_envelope(current,n,bound);
        if got {
            assert!(current<(1u128<<127));
            if n!=0 {
                let shift=64-(n-1).leading_zeros();
                assert!(bound.leading_zeros()>=shift);
                let rounded=1u128<<shift;
                assert!(rounded>=u128::from(n));
                let contribution=bound<<shift;
                assert_eq!(contribution>>shift,bound);
                assert!(current.checked_add(contribution).unwrap()<(1u128<<127));
            }
        }
        kani::cover!(got && n>0); kani::cover!(!got);
    }
    #[kani::proof]
    fn perf2c_reduced_sum_envelope() {
        let current:u16=kani::any(); let n:u8=kani::any(); let bound:u16=kani::any();
        let shift=if n==0 {0} else {8-(n-1).leading_zeros()};
        let admitted=u32::from(current).checked_add(u32::from(bound)<<shift).unwrap()<(1u32<<15);
        if admitted {
            assert!(u32::from(current)+u32::from(n)*u32::from(bound)<(1u32<<15));
        }
        kani::cover!(admitted && n>0); kani::cover!(!admitted);
    }

    #[kani::proof]
    fn perf2c_reduced_bit_product() {
        let (x,y):(i16,i16)=kani::any(); let (a,b):(u8,u8)=kani::any();
        kani::assume(a<=15 && b<=15);
        kani::assume(i32::from(x).unsigned_abs()<(1u32<<a));
        kani::assume(i32::from(y).unsigned_abs()<(1u32<<b));
        let v=i32::from(x)*i32::from(y);
        assert!(v.unsigned_abs()<(1u32<<(u32::from(a)+u32::from(b))));
        kani::cover!(x<0 && y<0); kani::cover!(x>0 && y>0);
    }
}
