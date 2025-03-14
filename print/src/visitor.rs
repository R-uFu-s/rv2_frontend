// Some headers
use rustc_middle::{
  mir::{Body},
  ty::{TyCtxt,Ty},
};
use rustc_hir::{StmtKind, Stmt, Local, Expr, ExprKind, UnOp, Param,
  QPath, Path, def::Res, PatKind, Mutability};
use rustc_utils::mir::mutability;
use std::{collections::HashMap};
use rustc_ast::walk_list;
use rustc_span::Span;
use aquascope::analysis::{
  boundaries::PermissionsBoundary};
use rustc_hir::{intravisit::{self, Visitor},hir_id::HirId};
use std::cmp::{Eq, PartialEq};
use std::hash::{Hash, Hasher};

#[derive(Eq, PartialEq, Hash)]
pub struct AccessPoint {
  mutability: Mutability,
  name:String,
}

#[derive(Eq, PartialEq,Hash)]
pub enum AccessPointUsage{
  Owner(AccessPoint),
  MutRef(AccessPoint),
  StaticRef(AccessPoint),
  Struct(Vec<AccessPoint>),
  Function(String),
  Block(AccessPoint),//this is a temporary solution, we need to justifiy the type of block by the last line of it
}

#[derive(Eq, PartialEq,Hash)]
pub enum Reference{
  Static(String),
  Mut(String),
}


// A small helper function
fn extract_var_name(input_string: &str ) -> Option<String> {
  let start_index = input_string.find('`')? + 1;
  let end_index = input_string.rfind('`')?;
  let rough_string=input_string[start_index..end_index].to_owned();
  if rough_string.contains("String::from"){
    Some(String::from("String::from"))
  }
  else{
    Some(rough_string)
  }
}


// Implement the visitor 
pub struct ExprVisitor<'a, 'tcx:'a> {
  pub tcx: TyCtxt<'tcx>,
  pub mir_body: &'a Body<'tcx>,
  pub boundary_map: HashMap<rustc_span::BytePos,PermissionsBoundary>,
  pub mutability_map: HashMap<String,Mutability>,
  pub lifetime_map: HashMap<Reference,usize>,
  pub borrow_map: HashMap<String, Option<String>>,
  pub access_points: HashMap<AccessPointUsage, usize>,
  pub current_scope: usize,
}

// These are helper functions used the visitor
impl<'a, 'tcx> ExprVisitor<'a, 'tcx>{
  fn expr_to_line(&self,expr:&Expr)->usize{
    self.tcx.sess.source_map().lookup_char_pos(expr.span.lo()).line
  }

  fn span_to_line(&self,span:&Span)->usize{
    self.tcx.sess.source_map().lookup_char_pos(span.lo()).line
  }

  fn hirid_to_var_name(&self,id:HirId)->Option<String>{
    let long_name = self.tcx.hir().node_to_string(id);
    extract_var_name(&long_name)
  }

  fn return_type_of(&self,fn_expr:&Expr)->Option<Ty<'tcx>>{
    let type_check = self.tcx.typeck(fn_expr.hir_id.owner);
    let type_of_path = type_check.expr_ty(fn_expr);
    let mut fn_sig = type_of_path.fn_sig(self.tcx).skip_binder().output().walk();
    if let Some(return_type)= fn_sig.next(){
      Some(return_type.expect_ty())
    }
    else {
      None
    }
  }

  fn is_return_type_ref(&self,fn_expr:&Expr)->bool{
    if let Some(return_type)=self.return_type_of(fn_expr){
      return_type.is_ref()
    }
    else{
      false
    }
  }

  fn is_return_type_copyable(&self,fn_expr:&Expr)->bool{
    if let Some(return_type)=self.return_type_of(fn_expr){
      if return_type.walk().fold(false,|flag,item|{flag||item.expect_ty().is_ref()}) {
        false
      }
      else{
        return_type.is_copy_modulo_regions(self.tcx, self.tcx.param_env(fn_expr.hir_id.owner))
      }
    }
    else{
      false
    }
  }

  fn update_lifetime(&mut self, reference:Reference, line:usize){
    if self.lifetime_map.contains_key(&reference){
      if let Some(old_line)=self.lifetime_map.get(&reference){
        if *old_line<line {
          self.lifetime_map.insert(reference, line);
        }
      }
    }
    else{
      self.lifetime_map.insert(reference, line);
    }
  }

  fn match_rhs(&mut self,lhs:AccessPoint,rhs:&Expr){
    let lhs_var=lhs.name.clone();
    let line_num = self.expr_to_line(rhs);
    match rhs.kind {
      ExprKind::Path(QPath::Resolved(_,p)) => {
        let bytepos=p.span.lo();
        let name = self.hirid_to_var_name(p.segments[0].hir_id);
        let boundary=self.boundary_map.get(&bytepos);
        if let Some(name)=name {
          if let Some(boundary) = boundary {
            if let Some(rhs_mut)=self.mutability_map.get(&name){
              let rhs = AccessPoint{mutability:*rhs_mut,name:name.clone()};
              if boundary.expected.drop {   
                println!("On line: {}\nMove({}->{})", line_num, name, lhs_var);
                if self.access_points.contains_key(&AccessPointUsage::Owner(rhs)){
                  self.access_points.insert(AccessPointUsage::Owner(lhs),self.current_scope);
                }
                else {
                  self.access_points.insert(AccessPointUsage::MutRef(lhs),self.current_scope);
                  if let Some(owner)=self.borrow_map.get(&name){
                    if let Some(owner)=owner{
                      self.borrow_map.insert(lhs_var.clone(),Some(owner.clone()));
                    }
                    else {
                      self.borrow_map.insert(lhs_var.clone(),None);
                    }
                  }
                  self.update_lifetime(Reference::Mut(name), line_num);
                  self.update_lifetime(Reference::Mut(lhs_var), line_num);
                }
              }
              else {
                println!("On line: {}\nCopy({}->{})", line_num, name, lhs_var);
                if self.access_points.contains_key(&AccessPointUsage::Owner(rhs)){
                  self.access_points.insert(AccessPointUsage::Owner(lhs),self.current_scope);
                }
                else {
                  self.access_points.insert(AccessPointUsage::StaticRef(lhs),self.current_scope);
                  if let Some(owner)=self.borrow_map.get(&name){
                    if let Some(owner)=owner{
                      self.borrow_map.insert(lhs_var.clone(),Some(owner.clone()));
                    }
                    else {
                      self.borrow_map.insert(lhs_var.clone(),None);
                    }
                  }
                  self.update_lifetime(Reference::Static(name), line_num);
                  self.update_lifetime(Reference::Static(lhs_var), line_num);
                }
              }
            }
          }
        }   
      },
      ExprKind::Call(fn_expr, _) => {
        let fn_name = self.hirid_to_var_name(fn_expr.hir_id);
        if let Some(fn_name) = fn_name {
          if !self.is_return_type_ref(fn_expr){
            if self.is_return_type_copyable(fn_expr) {
              println!("On line: {}\nCopy({}()->{})", line_num, fn_name, lhs_var);
            }
            else {
              println!("On line: {}\nMove({}()->{})", line_num, fn_name, lhs_var);
            }
            self.access_points.insert(AccessPointUsage::Owner(lhs),self.current_scope);
          }
          else {
            if let Some(return_type)=self.return_type_of(fn_expr){
              self.borrow_map.insert(lhs_var.clone(),None);
              if let Some(mutability)=return_type.ref_mutability(){
                match mutability{
                  Mutability::Mut=>{
                    println!("On line: {}\nMove({}()->{})", line_num, fn_name, lhs_var);
                    self.access_points.insert(AccessPointUsage::MutRef(lhs),self.current_scope);
                    self.update_lifetime(Reference::Mut(lhs_var), line_num);
                  }
                  Mutability::Not=>{
                    println!("On line: {}\nCopy({}()->{})", line_num, fn_name, lhs_var);
                    self.access_points.insert(AccessPointUsage::StaticRef(lhs),self.current_scope);
                    self.update_lifetime(Reference::Static(lhs_var), line_num);
                  }
                }
              }
            }
          }
          self.access_points.insert(AccessPointUsage::Function(fn_name),self.current_scope);
        }
      },
      ExprKind::Lit(_) => {
        println!("On line: {}\nBind({})", line_num, lhs_var);
        self.access_points.insert(AccessPointUsage::Owner(lhs),self.current_scope);
      }
      ExprKind::AddrOf(_,mutability,expr) => {
        match expr.kind{
          ExprKind::Path(QPath::Resolved(_,p))=>{
            if let Some(name)=self.hirid_to_var_name(p.segments[0].hir_id){
              self.borrow_map.insert(lhs_var.clone(),Some(name.clone()));
              match mutability{
                Mutability::Not=>{
                  println!("On line: {}\nStaticBorrow({}->{})", line_num, name,lhs_var);
                  self.update_lifetime(Reference::Static(lhs_var.clone()), line_num);
                  self.access_points.insert(AccessPointUsage::StaticRef(lhs),self.current_scope);
                }
                Mutability::Mut=>{
                  println!("On line: {}\nMutableBorrow({}->{})", line_num, name,lhs_var);
                  self.update_lifetime(Reference::Mut(lhs_var.clone()), line_num);
                  self.access_points.insert(AccessPointUsage::MutRef(lhs),self.current_scope);
                }
              }
            }
          }
          _=>{}
        }
      }
      ExprKind::Block(_, _) => {
        self.access_points.insert(AccessPointUsage::Block(lhs),self.current_scope);
      }
      _=>{}
    }
  }

  pub fn print_definitions(&self){
    println!();
    println!("/*--- BEGIN Variable Definitions ---");
    for (point,_) in &self.access_points {
      match point {
        AccessPointUsage::Owner(p)|
        AccessPointUsage::Block(p)
        =>{
          println!("Owner {:?} {};",p.mutability,p.name);
        }
        AccessPointUsage::StaticRef(p)=>{
          println!("StaticRef {:?} {};",p.mutability,p.name);
        }
        AccessPointUsage::MutRef(p)=>{
          println!("MutRef {:?} {};",p.mutability,p.name);
        }
        AccessPointUsage::Function(name)=>{
          println!("Function {}();",name);
        }
        _=>{}
      }
    }
    println!("--- END Variable Definitions ---*/");
  }

  pub fn print_out_of_scope(&self){
    println!();
    for (point,gos) in &self.access_points {
      if gos!=&0 {
        match point {
          AccessPointUsage::Owner(p)|
          AccessPointUsage::StaticRef(p)|
          AccessPointUsage::MutRef(p)=>{
            println!();
            println!("On line {}", gos);
            println!("GoOutOfScope({})",p.name);
          }
          _=>{}
        }
      }
    }
  }

  pub fn print_lifetimes(&self){
    println!();
    for (reference,line_num) in &self.lifetime_map{
      match reference{
        Reference::Mut(name)=>{
          if let Some(owner)=self.borrow_map.get(name){
            if let Some(owner)=owner{
              println!("On line {}\nMutableDie({}->{})", line_num, name,owner);
            }
            else {
              println!("On line {}\nMutableDie({}->*{})", line_num, name,name);
            }
          }
        }
        Reference::Static(name)=>{
          if let Some(owner)=self.borrow_map.get(name){
            if let Some(owner)=owner{
              println!("On line {}\nStaticDie({}->{})", line_num, name,owner);
            }
            else {
              println!("On line {}\nStaticDie({}->*{})", line_num, name,name);
            }
          }
        }
      }
    }
  }
}

// these are the visitor trait itself
// the visitor will walk through the hir
// the approach we are using is simple here. when visiting an expression or a statement,
// match it with a pattern and do analysis accordingly. The difference between expressions 
// and statements is subtle. As far as I can infer, everything is expression except "let a = b"
// is only found as statement.      
// See ExprKind at : https://doc.rust-lang.org/stable/nightly-rustc/rustc_hir/hir/enum.ExprKind.html
// See StmtKind at : https://doc.rust-lang.org/stable/nightly-rustc/rustc_hir/hir/enum.StmtKind.html
impl<'a, 'tcx> Visitor<'tcx> for ExprVisitor<'a, 'tcx> {
  fn visit_param(&mut self, param: &'tcx Param<'tcx>){
    let line_num=self.span_to_line(&param.span);
    let ty = self.tcx.typeck(param.hir_id.owner).pat_ty(param.pat);
    match param.pat.kind {
      PatKind::Binding(binding_annotation, ann_hirid, ident, op_pat) =>{
        let name = ident.to_string();
        let mutability = binding_annotation.1;
        if ty.is_ref() {
          println!("InitRefParam({})",name);
          if let Some(mutref)=ty.ref_mutability(){
            match mutref {
              Mutability::Not=>{
                self.update_lifetime(Reference::Static(name.clone()), line_num);
                self.access_points.insert(AccessPointUsage::StaticRef(AccessPoint { mutability, name}), self.current_scope);
              }
              Mutability::Mut=>{
                self.update_lifetime(Reference::Mut(name.clone()), line_num);
                self.access_points.insert(AccessPointUsage::MutRef(AccessPoint { mutability, name}), self.current_scope);
              }
            }
          }
        }
        else{
          println!("InitOwnerParam({})",name);
          self.access_points.insert(AccessPointUsage::Owner(AccessPoint { mutability, name}), self.current_scope);
        }
      }
      _=>{}
    }
  }
  fn visit_expr(&mut self, expr: &'tcx Expr<'tcx>) {
      let hirid = expr.hir_id;
        match expr.kind {
          ExprKind::Call(fn_expr, args) => {
            let line_num = self.expr_to_line(expr);
            println!();
            //println!("Function Call: {}", self.tcx.hir().node_to_string(hirid));
            //println!("On line: {}", line_num);
            // deal with println!
            let fn_name = self.hirid_to_var_name(fn_expr.hir_id);
            if let Some(fn_name) = fn_name {
              if fn_name.contains("crate::io::_print"){
                // args[0] is the format string: crate::format_args_nl!($($arg)*)
                match args[0].kind {
                  ExprKind::Call(format_expr, format_args)=>{
                    for a in format_args {
                      self.visit_expr(a);
                    }
                  }
                  _=>{} 
                }
              }
            }
            for arg in args.iter(){
              match arg.kind {
                ExprKind::Path(QPath::Resolved(_,p))=>{
                  let bytepos=p.span.lo();
                  let boundary=self.boundary_map.get(&bytepos);
                  if let Some(boundary) = boundary {
                    let expected=boundary.expected;
                    let name = self.hirid_to_var_name(p.segments[0].hir_id);
                    if let Some(name) = name {
                      if let Some(mut fn_name) = self.hirid_to_var_name(fn_expr.hir_id) {
                        if expected.drop{
                          println!("On line: {}\nMove({}->{}())", line_num, name, fn_name);
                        }
                        else if expected.write{                          
                          println!("On line: {}\nPassByMutableReference({}->{}())", line_num, name, fn_name);
                          self.update_lifetime(Reference::Mut(name), line_num);
                        }
                        else if expected.read{
                          println!("On line: {}\nPassByStaticReference({}->{}())", line_num, name, fn_name);
                          self.update_lifetime(Reference::Static(name), line_num);
                        }
                        self.access_points.insert(AccessPointUsage::Function(fn_name),self.current_scope);
                      }
                    }
                  }
                }
                ExprKind::AddrOf(_,mutability,expr)=>{
                  if let Some(mut fn_name) = self.hirid_to_var_name(fn_expr.hir_id){
                    
                    match expr.kind{
                      ExprKind::Path(QPath::Resolved(_,p))=>{
                        if let Some(name)=self.hirid_to_var_name(p.segments[0].hir_id){
                          if fn_name.contains("{") { // println
                            fn_name = "println".to_string();
                            let mut_reference=Reference::Mut(name.clone());
                            let sta_reference=Reference::Static(name.clone());
                            if self.lifetime_map.contains_key(&mut_reference) {
                              self.update_lifetime(mut_reference, line_num);
                            } else if self.lifetime_map.contains_key(&sta_reference) {
                              self.update_lifetime(sta_reference, line_num);
                            }
                          }
                          match mutability{
                            Mutability::Not=>{
                              println!("On line: {}\nPassByStaticReference({}->{}())", line_num, name,fn_name);
                            }
                            Mutability::Mut=>{
                              println!("On line: {}\nPassByMutableReference({}->{}())", line_num, name,fn_name);
                            }
                          }
                          self.access_points.insert(AccessPointUsage::Function(fn_name),self.current_scope);
                        }
                      }
                      _=>{}
                    }
                  }
                }
                _=>{}
              }
              // self.visit_expr(arg);
            }
          }
          ExprKind::MethodCall(_, rcvr, args, fn_span)
            if !fn_span.from_expansion()
              && rcvr.is_place_expr(|e| !matches!(e.kind, ExprKind::Lit(_))) =>
          {
            
             let hir_id=rcvr.hir_id;
            for a in args.iter() {
              self.visit_expr(a);
            }
          }
          ExprKind::Binary(_, lhs, rhs) => {
            self.visit_expr(lhs);
            self.visit_expr(rhs);
          }
          ExprKind::Lit(_) => {
          }
    
          ExprKind::AddrOf(_, _, inner)
            if inner.is_syntactic_place_expr() && !inner.span.from_expansion() =>
          {
          }
          ExprKind::Assign(
            lhs,
            rhs,
            _,
          ) => {
            println!();
            //println!("Assign Expression: {}", self.tcx.hir().node_to_string(hirid));
            //println!("On line: {}", self.expr_to_line(expr));
            let mut lhs_var = "".to_string();
            match lhs.kind {
              ExprKind::Path(QPath::Resolved(_,p)) => {
                let name = self.hirid_to_var_name(p.segments[0].hir_id);
                if let Some(name) = name {
                  lhs_var = name;
                }
              },
              _=>{}
            }
            if let Some(mutability)=self.mutability_map.get(&lhs_var){
              self.match_rhs(AccessPoint { mutability:*mutability, name: lhs_var }, rhs);
            }
            self.visit_expr(lhs);
            match rhs.kind {
              ExprKind::Path(_) => {},
              _=>{
                self.visit_expr(rhs);
              }
            }
          }
    
          ExprKind::Block(block, _) => {
            //println!("Block: {}", self.tcx.hir().node_to_string(hirid));
            let line = self.tcx.sess.source_map().lookup_char_pos(expr.span.hi()).line;
            // println!("line: {:?}", line);
            let pre_scope = self.current_scope;
            self.current_scope = line;
            self.visit_block(block);
            self.current_scope = pre_scope;
          }
    
          ExprKind::AssignOp(_, lhs, rhs) => {
            self.visit_expr(lhs);
          }

          ExprKind::Unary(UnOp::Deref, inner)
            if inner.is_syntactic_place_expr() && !inner.span.from_expansion() =>
          {
           let hir_id=inner.hir_id;
          }
          
          ExprKind::Path(QPath::Resolved(
            _,
            Path {
              span,
              res: Res::Local(_),
              ..
            },
          )) if !span.from_expansion() => {
            let bytepos=span.lo();
            let boundary=self.boundary_map.get(&bytepos);
            if let Some(boundary)=boundary{
              if boundary.expected.drop {
                let name = self.hirid_to_var_name(hirid);
                if let Some(name) = name {
                  println!();
                  println!("On line: {}", self.expr_to_line(expr));
                  println!("Move({}->none)", name);
                }
              }
            }
          }
          
          _ => {
            intravisit::walk_expr(self, expr);
          }
        }
      }
      fn visit_stmt(&mut self, statement: &'tcx Stmt<'tcx>) {
        match statement.kind {
          StmtKind::Local(ref local) => self.visit_local(local),
          StmtKind::Item(item) => self.visit_nested_item(item),
          StmtKind::Expr(ref expression) | StmtKind::Semi(ref expression) => {
              self.visit_expr(expression)
          }
        }
      }
      fn visit_local(&mut self, local: &'tcx Local<'tcx>) {
        println!();
        //println!("Statement: {}", self.tcx.hir().node_to_string(local.hir_id));
        //println!("on line: {:#?}", self.span_to_line(&local.span));
        match local.pat.kind {
          PatKind::Binding(binding_annotation, ann_hirid, ident, op_pat) => {
            let lhs_var = ident.to_string();
            self.mutability_map.insert(lhs_var.clone(), binding_annotation.1);
            match local.init {
              | Some(expr) => {
                  self.match_rhs(AccessPoint { mutability: binding_annotation.1, name: lhs_var}, expr);
                  match expr.kind {
                    ExprKind::Path(_) => {},
                    _=>{
                      self.visit_expr(expr);
                    }
                  }
              },
              | _ => {},
              };
          }
          PatKind::Path(QPath::Resolved(_,p)) => {
            println!("lhs path: {:?}", self.tcx.def_path_str(p.res.def_id()));
          }
          _ => {
            println!("lhs is not listed");
          }
        }
        
        
        //walk_list!(self, visit_expr, &local.init);
        if let Some(els) = local.els {
        self.visit_block(els);
        }
        walk_list!(self, visit_ty, &local.ty);
      }
      
}

