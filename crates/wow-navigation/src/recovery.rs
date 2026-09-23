use crate::service::NavigationError;
#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum Recovery { Replan, Backtrack, Stop, Fail }
#[derive(Clone, Debug)] pub struct RecoveryBudget { pub attempts: u8, pub max_attempts: u8, pub last_error: Option<NavigationError> }
impl RecoveryBudget {
    pub fn new(max_attempts:u8)->Self{Self{attempts:0,max_attempts,last_error:None}}
    pub fn record(&mut self,error:NavigationError)->Recovery{self.attempts=self.attempts.saturating_add(1);self.last_error=Some(error);if self.attempts>=self.max_attempts{Recovery::Fail}else{Recovery::Replan}}
}
