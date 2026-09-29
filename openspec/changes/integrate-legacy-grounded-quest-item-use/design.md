# Design

Quest item-use rules remain grounded in the AzerothCore catalog. Objective resolution requires an incomplete objective whose credit entry matches the rule and a live target whose entry matches the rule. The lane then requires an authoritative backpack item instance. Before use, it checks authoritative item-template metadata for the rule spell. Missing metadata triggers the shared item query and a wait; conflicting metadata blocks use. Valid use continues through shared action validation and movement, then waits for objective credit.
