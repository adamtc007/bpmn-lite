(workflow custody-cbu-onboarding
  (start-event :id start :next create-cbu)
  (service-task :id create-cbu :verb cbu.create :next type-decision)
  (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next add-fund)
    (flow :condition (= @cbu-type "corporate") :next add-corp)
    (flow :condition (= @cbu-type "trust")     :next add-trust))
  (service-task :id add-fund  :verb cbu.add-product :args (:product "CUSTODY_FUND")  :next add-im)
  (service-task :id add-corp  :verb cbu.add-product :args (:product "CUSTODY_CORP")  :next add-im)
  (service-task :id add-trust :verb cbu.add-product :args (:product "CUSTODY_TRUST") :next add-im)
  (service-task :id add-im    :verb instrument-matrix.attach :next end)
  (end-event :id end :status "Operational"))
